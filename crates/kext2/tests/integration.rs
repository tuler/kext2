// These tests use Unix file APIs and every decompressor.
#![cfg(all(unix, feature = "gzip", feature = "bzip2", feature = "xz", feature = "zstd"))]

mod common;

use common::*;
use std::io::Write;
use std::os::unix::fs::symlink;

/// A tiny tree: one group of 12 blocks, fixed block layout.
#[test]
fn worked_example() {
    let d = tmpdir();
    let t = p(d.path(), "t1");
    std::fs::create_dir_all(t.join("sub")).unwrap();
    std::fs::write(t.join("a.txt"), b"hello\n").unwrap();
    std::fs::hard_link(t.join("a.txt"), t.join("hard")).unwrap();
    symlink("a.txt", t.join("link")).unwrap();
    std::process::Command::new("mkfifo").arg(t.join("fifo")).status().unwrap();
    let img = p(d.path(), "x.img");
    kext2(&["-f", "-d", s(&t), s(&img)]).unwrap();
    let b = std::fs::read(&img).unwrap();
    assert_eq!(b.len(), 12 * 4096);
    let sb = &b[1024..2048];
    assert_eq!(u32_at(sb, 0x04), 12); // blocks
    assert_eq!(u32_at(sb, 0x00), 32); // inodes
    assert_eq!(u32_at(sb, 0x0C), 0); // free blocks
    assert_eq!(u32_at(sb, 0x10), 17); // free inodes: geometry rounding only
    assert_eq!(u32_at(sb, 0x14), 0); // first data block
    assert_eq!(u32_at(sb, 0x18), 2); // 4 KiB blocks
    assert_eq!(u32_at(sb, 0x20), 16); // blocks per group
    assert_eq!(u32_at(sb, 0x08), 0); // no reserved blocks
    assert_eq!(u16_at(sb, 0x38), 0xEF53);
    assert_eq!(u32_at(sb, 0x60), 2); // filetype
    assert_eq!(u32_at(sb, 0x64), 1); // sparse_super

    // GDT at block 1: bitmaps at 2, 3, inode table at 4
    let gd = &b[4096..4096 + 32];
    assert_eq!((u32_at(gd, 0), u32_at(gd, 4), u32_at(gd, 8)), (2, 3, 4));
    let ino = |n: usize| &b[4 * 4096 + (n - 1) * 128..4 * 4096 + n * 128];
    // root (inode 2) at block 5, lost+found (inode 11) at blocks 6-9
    assert_eq!(u32_at(ino(2), 40), 5);
    assert_eq!(u16_at(ino(2), 0), 0o40755);
    assert_eq!(u16_at(ino(11), 0), 0o40700);
    assert_eq!((u32_at(ino(11), 4), u32_at(ino(11), 28), u32_at(ino(11), 40)), (16384, 32, 6));
    // a.txt is the first child placed (inode 12) at block 10; sub at block 11
    let m = read_image(&img);
    assert_eq!(m["/a.txt"].data, b"hello\n");
    assert_eq!(m["/hard"].ino, m["/a.txt"].ino);
    assert_eq!(m["/link"].target, b"a.txt");
    assert_eq!(m["/fifo"].kind, 'p');
    assert_eq!(m["/sub"].kind, 'd');
    assert_eq!(m["/lost+found"].kind, 'd');
    assert_eq!(u32_at(ino(12), 40), 10);
    assert_eq!(u16_at(ino(12), 26), 2); // two names
    assert_eq!(u32_at(ino(15), 40), 11); // sub: last inode, last block
}

fn sample_tar(fmt: Fmt) -> Vec<u8> {
    let mut w = TarW::new();
    let h = |name: &'static str, tf: u8| H { name: name.as_bytes(), typeflag: tf, fmt, ..Default::default() };
    w.entry(H { mode: 0o711, uid: 5, gid: 6, ..h("./", b'5') }, b"");
    w.entry(h("dir/", b'5'), b"");
    w.entry(H { mode: 0o4755, uid: 1000, gid: 100, ..h("dir/file", b'0') }, b"some data");
    w.entry(H { link: b"dir/file", ..h("dir/hard", b'1') }, b"");
    w.entry(H { link: b"../x/y", mode: 0o755, ..h("sym", b'2') }, b"");
    w.entry(H { mtime: -1000, ..h("old", b'0') }, b"old");
    if fmt != Fmt::V7 {
        w.entry(H { major: 1, minor: 3, mode: 0o666, ..h("null", b'3') }, b"");
        w.entry(H { major: 8, minor: 1, ..h("sda1", b'4') }, b"");
        w.entry(h("fifo", b'6'), b"");
    }
    w.finish()
}

#[test]
fn tar_formats() {
    let d = tmpdir();
    for fmt in [Fmt::V7, Fmt::Ustar, Fmt::Gnu] {
        let tar = p(d.path(), "a.tar");
        std::fs::write(&tar, sample_tar(fmt)).unwrap();
        let img = p(d.path(), "a.img");
        kext2(&["-a", s(&tar), s(&img)]).unwrap();
        let m = read_image(&img);
        let f = &m["/dir/file"];
        assert_eq!(
            (f.data.as_slice(), f.perm, f.uid, f.gid, f.mtime),
            (&b"some data"[..], 0o4755, 1000, 100, 1_500_000_000)
        );
        assert_eq!(m["/dir/hard"].ino, f.ino);
        assert_eq!((m["/sym"].target.as_slice(), m["/sym"].perm), (&b"../x/y"[..], 0o777));
        assert_eq!(m["/old"].mtime, -1000);
        assert_eq!(m["/lost+found"].kind, 'd');
        if fmt != Fmt::V7 {
            assert_eq!((m["/null"].kind, m["/null"].dev), ('c', (1, 3)));
            assert_eq!((m["/sda1"].kind, m["/sda1"].dev), ('b', (8, 1)));
            assert_eq!(m["/fifo"].kind, 'p');
        }
        // the './' entry set the root's mode and owner
        let b = std::fs::read(&img).unwrap();
        let sb = &b[1024..2048];
        let it = u32_at(&b[4096..], 8) as usize;
        let root = &b[it * 4096 + 128..it * 4096 + 256];
        assert_eq!(u16_at(root, 0), 0o40711);
        assert_eq!((u16_at(root, 2), u16_at(root, 24)), (5, 6));
        assert_eq!(u32_at(root, 16), u32_at(sb, 0x30)); // root time = T_fs
    }
}

#[test]
fn tar_extensions() {
    let d = tmpdir();
    let mut w = TarW::new();
    let long = "L".repeat(200) + "/" + &"n".repeat(250);
    // GNU long name and long link
    w.gnu_long(b'L', long.as_bytes());
    w.entry(H { name: b"trunc", ..Default::default() }, b"long");
    w.gnu_long(b'K', "t".repeat(300).as_bytes());
    w.gnu_long(b'L', b"longlink");
    w.entry(H { name: b"x", typeflag: b'2', ..Default::default() }, b"");
    // pax path, big ids, fractional negative time
    w.pax(b'x', &[("path", b"pax/name\xc3\xa9"), ("uid", b"100000"), ("gid", b"4294967294"), ("mtime", b"-1.5")]);
    w.entry(H { name: b"ignored", fmt: Fmt::Ustar, ..Default::default() }, b"pax");
    // global header applies to later members
    w.pax(b'g', &[("uid", b"77")]);
    w.entry(H { name: b"g1", fmt: Fmt::Ustar, ..Default::default() }, b"1");
    w.entry(H { name: b"g2", uid: 3_000_000, fmt: Fmt::Ustar, ..Default::default() }, b"2");
    // ustar prefix
    let mut h = header(&H { name: b"file", size: 3, fmt: Fmt::Ustar, ..Default::default() });
    h[345..351].copy_from_slice(b"prefix");
    checksum(&mut h);
    w.raw(&h, b"pre");
    // members without parents, odd names
    w.file("./a/b/c/deep", b"deep");
    w.file("/abs//x", b"abs");
    w.file("../up", b"skipped");
    let tar = p(d.path(), "e.tar");
    std::fs::write(&tar, w.finish()).unwrap();
    let img = p(d.path(), "e.img");
    kext2(&["-a", s(&tar), s(&img)]).unwrap();
    let m = read_image(&img);
    let long_path = format!("/{long}");
    assert_eq!(m[&long_path].data, b"long");
    assert_eq!(m["/longlink"].target, "t".repeat(300).as_bytes());
    let pn = "/pax/name\u{e9}";
    assert_eq!((m[pn].uid, m[pn].gid, m[pn].mtime, m[pn].data.as_slice()), (100000, 4294967294, -2, &b"pax"[..]));
    assert_eq!(m["/g1"].uid, 77);
    assert_eq!(m["/g2"].uid, 77);
    assert_eq!(m["/prefix/file"].data, b"pre");
    assert_eq!(m["/a/b/c/deep"].data, b"deep");
    assert_eq!(m["/a/b"].perm, 0o755);
    assert_eq!(m["/abs/x"].data, b"abs");
    assert!(!m.contains_key("/up"));
}

/// Old GNU sparse ('S', with extension headers) and pax sparse 0.0, 0.1, 1.0.
#[test]
fn tar_sparse() {
    let d = tmpdir();
    let real: u64 = 70 * 1024 * 1024 + 5;
    // regions: 30 of them (more than 4 → GNU extension headers)
    let regions: Vec<(u64, u64)> = (0..30).map(|i| (i * 2 * 1024 * 1024 + i * 7, 100 + i * 13)).collect();
    let mut content = Vec::new();
    for (i, &(_, n)) in regions.iter().enumerate() {
        content.extend((0..n).map(|k| (k as u8).wrapping_mul(i as u8 + 3) | 1));
    }
    let stored = content.len() as u64;
    let expect = |m: &std::collections::BTreeMap<String, E>, name: &str| {
        let f = &m[name];
        assert_eq!(f.data.len() as u64, real, "{name}");
        let mut pos = 0usize;
        let mut last = 0u64;
        for &(o, n) in &regions {
            assert!(f.data[last as usize..o as usize].iter().all(|&b| b == 0));
            assert_eq!(&f.data[o as usize..(o + n) as usize], &content[pos..pos + n as usize]);
            pos += n as usize;
            last = o + n;
        }
        assert!(f.data[last as usize..].iter().all(|&b| b == 0));
    };
    let mut w = TarW::new();
    // old GNU
    {
        let mut h = header(&H { name: b"oldgnu", typeflag: b'S', size: stored, ..Default::default() });
        for (i, &(o, n)) in regions.iter().take(4).enumerate() {
            let e = 386 + i * 24;
            h[e..e + 12].copy_from_slice(format!("{:011o}\0", o).as_bytes());
            h[e + 12..e + 24].copy_from_slice(format!("{:011o}\0", n).as_bytes());
        }
        h[482] = 1;
        h[483..495].copy_from_slice(format!("{:011o}\0", real).as_bytes());
        checksum(&mut h);
        w.out.extend_from_slice(&h);
        let rest: Vec<_> = regions[4..].to_vec();
        let chunks: Vec<_> = rest.chunks(21).collect();
        for (ci, c) in chunks.iter().enumerate() {
            let mut x = [0u8; 512];
            for (i, &(o, n)) in c.iter().enumerate() {
                x[i * 24..i * 24 + 12].copy_from_slice(format!("{:011o}\0", o).as_bytes());
                x[i * 24 + 12..i * 24 + 24].copy_from_slice(format!("{:011o}\0", n).as_bytes());
            }
            x[504] = (ci + 1 < chunks.len()) as u8;
            w.out.extend_from_slice(&x);
        }
        w.out.extend_from_slice(&content);
        w.out.extend(std::iter::repeat_n(0u8, (512 - content.len() % 512) % 512));
    }
    // pax 0.0
    let mut recs: Vec<(String, Vec<u8>)> = vec![("GNU.sparse.size".into(), real.to_string().into_bytes())];
    for &(o, n) in &regions {
        recs.push(("GNU.sparse.offset".into(), o.to_string().into_bytes()));
        recs.push(("GNU.sparse.numbytes".into(), n.to_string().into_bytes()));
    }
    let r: Vec<(&str, &[u8])> = recs.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    w.pax(b'x', &r);
    w.entry(H { name: b"pax00", fmt: Fmt::Ustar, ..Default::default() }, &content);
    // pax 0.1
    let map: Vec<String> = regions.iter().map(|&(o, n)| format!("{o},{n}")).collect();
    let map = map.join(",");
    w.pax(
        b'x',
        &[
            ("GNU.sparse.size", real.to_string().as_bytes()),
            ("GNU.sparse.map", map.as_bytes()),
            ("GNU.sparse.name", b"pax01"),
        ],
    );
    w.entry(H { name: b"GNUSparseFile.1/pax01", fmt: Fmt::Ustar, ..Default::default() }, &content);
    // pax 1.0
    let mut data = format!("{}\n", regions.len()).into_bytes();
    for &(o, n) in &regions {
        data.extend_from_slice(format!("{o}\n{n}\n").as_bytes());
    }
    data.resize(data.len().div_ceil(512) * 512, 0);
    data.extend_from_slice(&content);
    w.pax(
        b'x',
        &[
            ("GNU.sparse.major", b"1"),
            ("GNU.sparse.minor", b"0"),
            ("GNU.sparse.name", b"pax10"),
            ("GNU.sparse.realsize", real.to_string().as_bytes()),
        ],
    );
    w.entry(H { name: b"GNUSparseFile.1/pax10", fmt: Fmt::Ustar, ..Default::default() }, &data);
    let tar = p(d.path(), "s.tar");
    std::fs::write(&tar, w.finish()).unwrap();
    let img = p(d.path(), "s.img");
    kext2(&["-a", s(&tar), s(&img)]).unwrap();
    let m = read_image(&img);
    for n in ["/oldgnu", "/pax00", "/pax01", "/pax10"] {
        expect(&m, n);
    }
    // the data is stored dense
    assert!(std::fs::metadata(&img).unwrap().len() > 4 * real);
}

#[test]
fn compressed_and_threads_identical() {
    let d = tmpdir();
    let tar = sample_tar(Fmt::Gnu);
    let mut variants: Vec<(String, Vec<u8>)> = vec![("plain".into(), tar.clone())];
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar[..1000]).unwrap();
    let mut first = gz.finish().unwrap();
    let mut gz2 = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz2.write_all(&tar[1000..]).unwrap();
    first.extend(gz2.finish().unwrap()); // two gzip members
    variants.push(("gz".into(), first));
    let mut bz = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
    bz.write_all(&tar).unwrap();
    variants.push(("bz2".into(), bz.finish().unwrap()));
    let mut xz = liblzma::write::XzEncoder::new(Vec::new(), 6);
    xz.write_all(&tar).unwrap();
    variants.push(("xz".into(), xz.finish().unwrap()));
    variants.push(("zst".into(), zstd::encode_all(&tar[..], 3).unwrap()));
    let mut images = Vec::new();
    for (i, (name, bytes)) in variants.iter().enumerate() {
        let t = p(d.path(), &format!("in-{name}"));
        std::fs::write(&t, bytes).unwrap();
        let img = p(d.path(), &format!("o{i}.img"));
        kext2_threads(&["-a", s(&t), "--free-space", "10M", s(&img)], Some(1 + i % 3)).unwrap();
        images.push(std::fs::read(&img).unwrap());
    }
    for im in &images[1..] {
        assert!(im == &images[0]);
    }
}

#[test]
fn layering_first_wins_and_paths() {
    let d = tmpdir();
    let a = p(d.path(), "A");
    let b = p(d.path(), "B");
    std::fs::create_dir_all(a.join("etc")).unwrap();
    std::fs::create_dir_all(b.join("etc")).unwrap();
    std::fs::write(a.join("etc/conf"), b"from A").unwrap();
    std::fs::write(b.join("etc/conf"), b"from B").unwrap();
    std::fs::write(b.join("etc/extra"), b"only B").unwrap();
    std::fs::write(b.join("lost+found"), b"a file").unwrap();
    let mut w = TarW::new();
    w.file("bin/sh", b"tar sh");
    let tar = p(d.path(), "c.tar");
    std::fs::write(&tar, w.finish()).unwrap();
    let dt = p(d.path(), "dt");
    std::fs::write(
        &dt,
        "/etc/conf f 600 7 8\n/sub/dev d 755 0 0\n/sub/dev/tty c 620 0 5 4 1 0 1 3\n/missing/x f 644\n",
    )
    .unwrap();
    let img = p(d.path(), "l.img");
    let la = s(&a).to_string();
    let lb = s(&b).to_string() + ":/";
    let lc = s(&tar).to_string() + ":/sub/./dir/";
    let ld = s(&dt).to_string();
    kext2(&["-d", &la, "-d", &lb, "-a", &lc, "-D", &ld, s(&img)]).unwrap();
    let m = read_image(&img);
    assert_eq!(m["/etc/conf"].data, b"from A");
    assert_eq!((m["/etc/conf"].perm, m["/etc/conf"].uid, m["/etc/conf"].gid), (0o600, 7, 8));
    assert_eq!(m["/etc/extra"].data, b"only B");
    assert_eq!(m["/sub/dir/bin/sh"].data, b"tar sh");
    assert_eq!((m["/sub/dir"].perm, m["/sub/dir"].uid), (0o755, 0));
    // the input file lost+found loses against the generated directory
    assert_eq!((m["/lost+found"].kind, m["/lost+found"].perm), ('d', 0o700));
    assert_eq!((m["/sub/dev/tty0"].dev, m["/sub/dev/tty2"].dev), ((4, 1), (4, 3)));
    assert!(!m.contains_key("/sub/dev/tty3"));
    assert!(!m.contains_key("/missing"));
}

/// A file mapped through the triple-indirect block: more than 4 GiB, built
/// from a sparse tar member so that the image stays sparse.
#[test]
fn triple_indirect() {
    let d = tmpdir();
    let pp: u64 = 1024;
    let marks = [0, 11, 12, 12 + pp - 1, 12 + pp, 12 + pp + pp * pp - 1, 12 + pp + pp * pp, 12 + pp + pp * pp + 70000];
    let real = (marks[marks.len() - 1] + 1) * 4096;
    let mut content = Vec::new();
    let mut map = Vec::new();
    for &l in &marks {
        content.extend_from_slice(&(l + 1).to_le_bytes());
        map.push(format!("{},8", l * 4096 + 100));
    }
    let map = map.join(",");
    let mut w = TarW::new();
    w.pax(
        b'x',
        &[
            ("GNU.sparse.size", real.to_string().as_bytes()),
            ("GNU.sparse.map", map.as_bytes()),
            ("GNU.sparse.name", b"big"),
        ],
    );
    w.entry(H { name: b"GNUSparseFile.1/big", fmt: Fmt::Ustar, ..Default::default() }, &content);
    let tar = p(d.path(), "t.tar");
    std::fs::write(&tar, w.finish()).unwrap();
    let img = p(d.path(), "h.img");
    kext2(&["-a", s(&tar), s(&img)]).unwrap();
    let m = read_image(&img);
    let f = &m["/big"];
    let nblocks = real / 4096;
    assert_eq!(f.map.iter().map(|r| r.2).sum::<u64>(), nblocks);
    let phys = |l: u64| {
        let r = f.map.iter().find(|r| r.0 <= l && l < r.0 + r.2).unwrap();
        r.1 + (l - r.0)
    };
    let file = std::fs::File::open(&img).unwrap();
    use std::os::unix::fs::FileExt;
    for &l in &marks {
        let mut v = [0u8; 8];
        file.read_exact_at(&mut v, phys(l) * 4096 + 100).unwrap();
        assert_eq!(v, (l + 1).to_le_bytes(), "block {l}");
    }
    // the triple-indirect pointer is set
    let b = std::fs::read(&img).map(|b| b[..1 << 20].to_vec()).unwrap();
    let it = u32_at(&b[4096..], 8) as usize;
    let off = it * 4096 + (f.ino as usize - 1) * 128;
    assert_ne!(u32_at(&b[off..], 40 + 14 * 4), 0);
}

#[test]
fn lost_found_and_big_directory() {
    let d = tmpdir();
    let t = p(d.path(), "t");
    std::fs::create_dir_all(t.join("big")).unwrap();
    for i in 0..3000 {
        std::fs::write(t.join("big").join(format!("entry-{i:05}-{}", "x".repeat(i % 50))), [i as u8]).unwrap();
    }
    let img = p(d.path(), "b.img");
    kext2(&["-d", s(&t), s(&img)]).unwrap();
    let m = read_image(&img);
    assert_eq!(m.keys().filter(|k| k.starts_with("/big/")).count(), 3000);
    assert_eq!(m["/big/entry-01234-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"].data, [(1234 % 256) as u8]);
    let lf = &m["/lost+found"];
    assert_eq!((lf.kind, lf.perm, lf.ino), ('d', 0o700, 11));
    let b = std::fs::read(&img).unwrap();
    let it = u32_at(&b[4096..], 8) as usize;
    let lfi = &b[it * 4096 + 10 * 128..it * 4096 + 11 * 128]; // inode 11
    assert_eq!(u16_at(lfi, 0), 0o40700);
    assert_eq!(u32_at(lfi, 4), 4 * 4096);
    assert_eq!(u32_at(lfi, 28), 4 * 8); // 4 data blocks, no indirect block

    // the 3000 entries take several blocks
    let big = &b[it * 4096 + (m["/big"].ino as usize - 1) * 128..];
    assert!(u32_at(big, 4) > 4096);
}

#[test]
fn errors_leave_no_output() {
    let d = tmpdir();
    let t = p(d.path(), "t");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(t.join("f"), vec![7u8; 100_000]).unwrap();
    let img = p(d.path(), "e.img");
    kext2(&["-d", s(&t), s(&img)]).unwrap();
    let good = std::fs::read(&img).unwrap();
    let mut w = TarW::new();
    w.entry(H { name: b"l", typeflag: b'2', link: &[b'x'; 100], ..Default::default() }, b"");
    w.gnu_long(b'K', &[b'y'; 4096]);
    w.entry(H { name: b"l2", typeflag: b'2', ..Default::default() }, b"");
    let tar = p(d.path(), "bad.tar");
    std::fs::write(&tar, w.finish()).unwrap();
    let img2 = p(d.path(), "e2.img");
    assert!(kext2(&["-a", s(&tar), s(&img2)]).unwrap_err().contains("symlink target length 4096"));
    assert!(!img2.exists());
    // a failed run leaves an earlier image untouched
    assert!(kext2(&["-d", s(&t), "-a", s(&tar), s(&img)]).is_err());
    assert!(std::fs::read(&img).unwrap() == good);
    assert_eq!(
        std::fs::read_dir(d.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("kext2"))
            .count(),
        0
    );
}

#[test]
fn reproducible_across_input_order() {
    let d = tmpdir();
    let mk = |name: &str, rev: bool| {
        let t = p(d.path(), name);
        std::fs::create_dir_all(&t).unwrap();
        let mut names: Vec<usize> = (0..200).collect();
        if rev {
            names.reverse();
        }
        for i in names {
            let sub = t.join(format!("d{}", i % 7));
            std::fs::create_dir_all(&sub).unwrap();
            let f = sub.join(format!("f{i}"));
            std::fs::write(&f, vec![i as u8; i * 37]).unwrap();
        }
        for i in 0..7 {
            let sub = t.join(format!("d{i}"));
            let tm = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i);
            std::fs::File::open(&sub).unwrap().set_modified(tm).unwrap();
        }
        t
    };
    let a = mk("a", false);
    let b = mk("bb", true);
    let ia = p(d.path(), "a.img");
    let ib = p(d.path(), "b.img");
    kext2(&["--clamp-timestamps", "1000005", "-d", s(&a), s(&ia)]).unwrap();
    kext2(&["--clamp-timestamps", "1000005", "-d", s(&b), s(&ib)]).unwrap();
    assert!(std::fs::read(&ia).unwrap() == std::fs::read(&ib).unwrap());
}

/// `--free-space` adds whole free blocks and one free inode per 16 KiB.
#[test]
fn free_space() {
    let d = tmpdir();
    let t = p(d.path(), "t");
    std::fs::create_dir_all(&t).unwrap();
    std::fs::write(t.join("f"), vec![1u8; 50_000]).unwrap();
    let img = p(d.path(), "r.img");
    // (blocks, free blocks, free inodes)
    let build = |a: &[&str]| -> (u64, u64, u64) {
        let mut v = vec!["-f"];
        v.extend_from_slice(a);
        v.extend_from_slice(&["-d", s(&t), s(&img)]);
        kext2(&v).unwrap();
        let b = std::fs::read(&img).unwrap();
        assert_eq!(b.len() as u64, u32_at(&b[1024..2048], 0x04) as u64 * 4096);
        let sb = &b[1024..2048];
        (u32_at(sb, 0x04) as u64, u32_at(sb, 0x0C) as u64, u32_at(sb, 0x10) as u64)
    };
    let (auto, free, _) = build(&[]);
    assert_eq!(free, 0);
    assert_eq!(build(&["--free-space", "0"]).0, auto);
    let (b1, free, inodes) = build(&["--free-space", "1"]);
    assert!(b1 > auto && free >= 1 && inodes >= 1);
    let (b4, free, inodes) = build(&["--free-space", "4M"]);
    assert!((1024..1024 + 8).contains(&free), "free {free}");
    assert!(inodes >= 256, "free inodes {inodes}");
    // 1024 free blocks and 8 more inode-table blocks for the 256 inodes
    assert!(b4 >= auto + 1024 + 8 && b4 <= auto + 1024 + 16, "{auto} {b4}");
    let (b5, free, _) = build(&["--free-space", "4194305"]);
    assert_eq!((b5 - b4, free), (1, 1025));
}
