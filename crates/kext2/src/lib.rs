//! kext2: build ext2 filesystem images from host directories, tar archives
//! and device tables, without root privileges.
//!
//! The work is done in three phases (SPEC §3): [`input`] builds an in-memory
//! [`tree`], [`plan`] fixes the geometry and the place of every inode and
//! block, and [`write`] renders the image in block order.

// Style lints whose suggestions need very recent toolchains or hurt clarity.
#![allow(unknown_lints, clippy::needless_late_init, clippy::chunks_exact_to_as_chunks)]

pub mod blockmap;
pub mod cli;
pub mod dirent;
pub mod error;
pub mod format;
pub mod geometry;
pub mod host;
pub mod image;
pub mod input;
pub mod layout;
#[cfg(feature = "memstats")]
pub mod memstats;
pub mod plan;
pub mod runs;
pub mod tree;
pub mod write;

use cli::{Action, LayerKind, Options, Readjust, TimeMode};
use error::{info, warn, Error, Result};
use input::Ctx;
use std::ffi::OsString;
use tree::{Node, Tree, ROOT};

/// Filesystem time `T_fs` (R-TIME-1).
pub fn filesystem_time(time: TimeMode, sde: Option<OsString>) -> Result<i64> {
    match time {
        TimeMode::Fake => Ok(0),
        TimeMode::Fixed(s) | TimeMode::Clamp(s) => Ok(s),
        TimeMode::Default => {
            if let Some(v) = sde.filter(|v| !v.is_empty()) {
                let b = host::os_bytes(&v);
                return cli::parse_num('S', &b, 0, i32::MAX as u64)
                    .map(|v| v as i64)
                    .map_err(|_| Error::fatal(format!("invalid SOURCE_DATE_EPOCH '{}'", String::from_utf8_lossy(&b))));
            }
            let now =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            Ok(now.min(i32::MAX as u64) as i64)
        }
    }
}

fn file_id(p: &std::ffi::OsStr) -> Option<(u64, u64)> {
    std::fs::metadata(p).ok().and_then(|m| host::file_id(&m))
}

/// Summary of a built image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub block_size: u32,
    pub blocks: u64,
    pub free_blocks: u64,
    pub reserved_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
    pub groups: u64,
}

/// Build the image described by `o`. `sde` is the value of `SOURCE_DATE_EPOCH`.
pub fn build(o: &Options, sde: Option<OsString>) -> Result<Summary> {
    build_with(o, sde, None)
}

/// [`build`] with an explicit number of writer threads (`None`: automatic).
pub fn build_with(o: &Options, sde: Option<OsString>, threads: Option<usize>) -> Result<Summary> {
    let t_fs = filesystem_time(o.time, sde)?;
    let t0 = std::time::Instant::now();
    let out_is_stdout = o.image == "-";
    let out_md = if out_is_stdout { None } else { std::fs::metadata(&o.image).ok() };
    let output_id = out_md.as_ref().and_then(host::file_id);
    let out_regular = out_md.as_ref().map(|m| m.is_file()).unwrap_or(!out_is_stdout);

    let mut tree = Tree::new(t_fs);
    let mut bs = o.block_size;
    let mut label = o.label.clone().unwrap_or_default();
    let mut creator_os = o.creator_os;
    let mut uuid = [0u8; 16];
    let mut blocks = o.blocks;
    let mut min_inodes = o.inodes.unwrap_or(0);
    let mut fixed_reserved = None;
    let mut bytes_per_inode = o.bytes_per_inode;
    // -r (SPEC Appendix C.1): +N adds free blocks, N means exactly N blocks;
    // ignored with -b.
    let mut readjust = 0;
    match (o.readjust, o.blocks) {
        (Some(_), Some(_)) => warn("-r is ignored when -b is given"),
        (Some(Readjust::Extra(n)), None) => readjust = n,
        (Some(Readjust::Absolute(n)), None) => blocks = Some(n),
        (None, _) => {}
    }

    if let Some(x) = &o.starting_image {
        // The starting image may be the output when the output is a regular
        // file: it is read through an open descriptor and replaced by rename.
        if output_id.is_some() && file_id(x) == output_id && !out_regular {
            return Err(Error::fatal("the starting image is also the output"));
        }
        info(format!("starting from image {}", x.to_string_lossy()));
        let ip = image::load(&mut tree, x, o.holes)?;
        let ignored = [
            (o.block_size_set, "-B"),
            (o.blocks.is_some(), "-b"),
            (o.inodes.is_some(), "-N"),
            (o.bytes_per_inode.is_some(), "-i"),
            (o.reserved_set, "-m"),
            (o.readjust.is_some(), "-r"),
            (o.label.is_some(), "-L"),
            (o.creator_os_set, "-o"),
        ];
        for (given, opt) in ignored {
            if given {
                warn(format!("{opt} is ignored with -x (taken from the starting image)"));
            }
        }
        bs = ip.bs;
        blocks = Some(ip.blocks);
        min_inodes = ip.inodes;
        fixed_reserved = Some(ip.reserved);
        bytes_per_inode = None;
        readjust = 0;
        let end = ip.label.iter().position(|&b| b == 0).unwrap_or(16);
        label = ip.label[..end].to_vec();
        creator_os = ip.creator_os;
        uuid = ip.uuid;
    } else if o.reserved_pct > 0 {
        let mut lf = Node::dir(0o700, 0, 0, t_fs);
        lf.generated = true;
        let id = tree.push(lf);
        tree.link_child(ROOT, b"lost+found", id);
    }

    let mut ctx = Ctx {
        tree,
        bs,
        holes: o.holes,
        squash_uid: o.squash_uid,
        squash_perms: o.squash_perms,
        host_links: Default::default(),
        output_id: if out_regular { output_id } else { None },
    };
    for l in &o.layers {
        if l.kind != LayerKind::Dir && l.src != "-" && output_id.is_some() && file_id(&l.src) == output_id {
            return Err(Error::fatal(format!("{} is also the output image", l.src.to_string_lossy())));
        }
        let comps: Vec<&[u8]> = l.path.iter().map(|c| &c[..]).collect();
        let joined = l.path.join(&b'/');
        let target = ctx.tree.ensure_dirs(ROOT, &comps, &joined)?;
        let src = l.src.to_string_lossy();
        match l.kind {
            LayerKind::Dir => {
                info(format!("copying from directory {src}"));
                input::dir::add_dir(&mut ctx, std::path::Path::new(&l.src), target)?;
            }
            LayerKind::Tar => {
                info(format!("copying from tar archive {src}"));
                input::tar::add_tar(&mut ctx, &l.src, target)?;
            }
            LayerKind::DevTable => {
                info(format!("nodes fixup and creation from device table {src}"));
                input::devtable::add_devtable(&mut ctx, &l.src, target, t_fs)?;
            }
        }
    }
    let mut tree = ctx.tree;
    // the scan is over: release the vectors' growth slack before planning
    tree.nodes.shrink_to_fit();
    tree.host_dirs.shrink_to_fit();

    let params = plan::Params {
        bs,
        blocks,
        readjust,
        bytes_per_inode,
        min_inodes,
        reserved_pct: o.reserved_pct,
        fixed_reserved,
        filetype: creator_os != 1,
    };
    let t_scan = t0.elapsed();
    #[cfg(feature = "memstats")]
    memstats::report("scan");
    let plan = plan::plan(&mut tree, &params)?;
    let t_plan = t0.elapsed();
    #[cfg(feature = "memstats")]
    memstats::report("plan");
    for p in &o.block_maps {
        if blockmap::resolve(&tree, &plan, p).is_none() {
            return Err(Error::fatal(format!("path {} not found in filesystem", error::show(p))));
        }
    }
    let mut lab = [0u8; 16];
    lab[..label.len().min(16)].copy_from_slice(&label[..label.len().min(16)]);
    let settings = write::Settings { t_fs, time: o.time, fill: o.fill, creator_os, label: lab, uuid };
    let r = write::Renderer::new(&tree, &plan, &settings);
    if o.verbose {
        describe(&r);
    }
    let sb = r.superblock();
    let summary = Summary {
        block_size: plan.geo.bs,
        blocks: plan.geo.blocks,
        free_blocks: sb.free_blocks_count as u64,
        reserved_blocks: plan.reserved,
        inodes: plan.geo.inodes_count(),
        free_inodes: sb.free_inodes_count as u64,
        groups: plan.geo.groups,
    };
    match threads {
        Some(t) => write::write_image_threads(&r, &o.image, t)?,
        None => write::write_image(&r, &o.image)?,
    }
    #[cfg(feature = "memstats")]
    memstats::report("write");
    blockmap::write_all(&tree, &plan, &o.block_maps)?;
    if o.verbose {
        let t_write = t0.elapsed();
        info(format!(
            "{} nodes; scan {:.3} s, plan {:.3} s, write {:.3} s",
            tree.nodes.len(),
            t_scan.as_secs_f64(),
            (t_plan - t_scan).as_secs_f64(),
            (t_write - t_plan).as_secs_f64()
        ));
    }
    Ok(summary)
}

fn describe(r: &write::Renderer) {
    let p = r.plan;
    let g = &p.geo;
    let sb = r.superblock();
    info(format!("block size {}, {} blocks, {} groups of {} blocks", g.bs, g.blocks, g.groups, g.bpg));
    info(format!(
        "{} inodes ({} per group, {} inode-table blocks per group), {} used",
        g.inodes_count(),
        g.ipg,
        g.itb,
        g.inodes_count() - sb.free_inodes_count as u64
    ));
    info(format!(
        "{} blocks of content, {} free, {} reserved, lost+found {}",
        p.total_blocks_used,
        sb.free_blocks_count,
        p.reserved,
        if p.lost_found.is_some() { "yes" } else { "no" }
    ));
    info(format!(
        "features: {}sparse_super{}",
        if p.filetype { "filetype " } else { "" },
        if p.large_file { " large_file" } else { "" }
    ));
}

/// Entry point used by the binary: returns the exit status.
pub fn main_with_args(args: Vec<OsString>) -> i32 {
    match cli::parse(args) {
        Ok(Action::Help) => {
            eprint!("{}", cli::USAGE);
            0
        }
        Ok(Action::Version) => {
            println!("kext2 {}", cli::VERSION);
            0
        }
        Ok(Action::Run(o)) => match build(&o, std::env::var_os("SOURCE_DATE_EPOCH")) {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("kext2: {}", e.msg);
                1
            }
        },
        Err(e) => {
            eprintln!("kext2: {}", e.msg);
            if e.usage {
                eprintln!("Try 'kext2 --help' for more information.");
            }
            1
        }
    }
}
