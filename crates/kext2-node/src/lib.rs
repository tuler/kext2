//! Node.js binding for kext2.
//!
//! Typed options are turned into kext2 command line arguments, so the binding
//! and the `kext2` tool validate them the same way. In-memory archives and
//! device tables, and in-memory images, go through temporary files.

use std::cell::RefCell;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use kext2::cli::{self, Action, Layer, LayerKind, Options};
use kext2::error::{with_messages, Level};
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// One input added to the image, in order.
#[napi(object)]
pub struct LayerSpec {
    /// `"dir"`, `"tar"` or `"devtable"`.
    #[napi(js_name = "type")]
    pub kind: String,
    /// Path of the directory, archive or device table.
    pub path: Option<String>,
    /// In-memory archive or device table (instead of `path`).
    pub data: Option<Buffer>,
    /// Directory inside the image to add the content to (default `/`).
    pub target: Option<String>,
}

/// Image description; mirrors the command line options.
#[napi(object)]
#[derive(Default)]
pub struct BuildOptions {
    /// Free space to leave in the image, in bytes (`--free-space`), with one
    /// free inode per 16 KiB of it.
    pub free_space: Option<i64>,
    /// Volume label (`-L`, up to 16 bytes).
    pub volume_label: Option<String>,
    /// Every timestamp is 0 (`-f`).
    pub faketime: Option<bool>,
    /// Every timestamp (filesystem and files) is this value.
    pub timestamp: Option<i64>,
    /// Filesystem time is this value; later file times are clamped to it.
    pub clamp_timestamps: Option<i64>,
    /// Filesystem time when no other timestamp option is given (defaults
    /// to the `SOURCE_DATE_EPOCH` environment variable, then now).
    pub source_date_epoch: Option<i64>,
    /// Make every file owned by this uid:gid (`-U`).
    pub squash_uids: Option<u32>,
    /// Remove group and other permissions (`-P`).
    pub squash_perms: Option<bool>,
    /// Content layers, applied in order.
    pub layers: Option<Vec<LayerSpec>>,
    /// Writer threads (default: automatic). The image does not depend on it.
    pub threads: Option<u32>,
    /// Report geometry details in `messages` (`-v`).
    pub verbose: Option<bool>,
}

/// A diagnostic message.
#[napi(object)]
pub struct Message {
    /// `"info"` or `"warning"`.
    pub level: String,
    pub text: String,
}

/// Summary of a built image.
#[napi(object)]
pub struct BuildResult {
    /// Always 4096.
    pub block_size: u32,
    pub blocks: i64,
    pub free_blocks: i64,
    pub inodes: i64,
    pub free_inodes: i64,
    pub groups: i64,
    pub messages: Vec<Message>,
}

/// Outcome of [`kext2_cli`].
#[napi(object)]
pub struct RunResult {
    /// What the command line tool would print on standard output (`--version`).
    pub stdout: String,
    /// Diagnostics, as the command line tool prints them on standard error.
    pub stderr: String,
    /// The image summary, when one was built.
    pub result: Option<BuildResult>,
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::new(Status::GenericFailure, e.to_string())
}

/// A fully parsed build: kext2 options plus what the binding keeps alive.
struct Job {
    options: Options,
    sde: Option<OsString>,
    threads: Option<usize>,
    /// Temporary files holding in-memory layers; removed on drop.
    _temps: Vec<tempfile::TempPath>,
}

fn layer_kind(kind: &str) -> Result<LayerKind> {
    match kind {
        "dir" | "directory" => Ok(LayerKind::Dir),
        "tar" | "tarball" => Ok(LayerKind::Tar),
        "devtable" => Ok(LayerKind::DevTable),
        other => Err(err(format!("unknown layer type '{other}'"))),
    }
}

fn job(o: BuildOptions, output: &Path) -> Result<Job> {
    let mut args: Vec<OsString> = Vec::new();
    let mut opt = |flag: &str, value: String| {
        args.push(flag.into());
        args.push(value.into());
    };
    if let Some(v) = o.free_space {
        opt("--free-space", v.to_string());
    }
    if let Some(v) = o.volume_label {
        opt("--volume-label", v);
    }
    if let Some(v) = o.timestamp {
        opt("--timestamp", v.to_string());
    }
    if let Some(v) = o.clamp_timestamps {
        opt("--clamp-timestamps", v.to_string());
    }
    if let Some(v) = o.squash_uids {
        opt("--squash-uids", v.to_string());
    }
    for (flag, on) in [("--faketime", o.faketime), ("--squash-perms", o.squash_perms), ("--verbose", o.verbose)] {
        if on == Some(true) {
            args.push(flag.into());
        }
    }
    args.push(output.as_os_str().to_owned());
    let mut options = match cli::parse(args).map_err(err)? {
        Action::Run(o) => *o,
        _ => return Err(err("unexpected help or version request")),
    };

    // Layers are added directly (not as `src:path` arguments), so paths may
    // contain ':'.
    let mut temps = Vec::new();
    for l in o.layers.unwrap_or_default() {
        let kind = layer_kind(&l.kind)?;
        let src: OsString = match (l.path, l.data) {
            (Some(p), None) => p.into(),
            (None, Some(d)) => {
                if kind == LayerKind::Dir {
                    return Err(err("directory layers need a path"));
                }
                let mut f = tempfile::NamedTempFile::new().map_err(err)?;
                f.write_all(&d).map_err(err)?;
                let path = f.into_temp_path();
                let src = path.as_os_str().to_owned();
                temps.push(path);
                src
            }
            _ => return Err(err("a layer needs exactly one of path or data")),
        };
        let target = l.target.unwrap_or_default();
        let path = cli::parse_image_path(target.as_bytes())
            .map_err(|_| err(format!("invalid layer target '{target}': '..' is not allowed")))?;
        options.layers.push(Layer { kind, src, path });
    }
    let sde = match o.source_date_epoch {
        Some(v) => Some(OsString::from(v.to_string())),
        None => std::env::var_os("SOURCE_DATE_EPOCH"),
    };
    Ok(Job { options, sde, threads: o.threads.map(|t| t as usize), _temps: temps })
}

/// Run a build on the current thread, collecting its messages.
fn run(job: &Job) -> std::result::Result<BuildResult, String> {
    let messages = Rc::new(RefCell::new(Vec::new()));
    let sink = messages.clone();
    let r = with_messages(
        move |level, text| {
            sink.borrow_mut().push(Message {
                level: match level {
                    Level::Info => "info".to_string(),
                    Level::Warning => "warning".to_string(),
                },
                text: text.to_string(),
            })
        },
        || kext2::build_with(&job.options, job.sde.clone(), job.threads),
    );
    let messages = Rc::try_unwrap(messages).map(RefCell::into_inner).unwrap_or_default();
    match r {
        Ok(s) => Ok(BuildResult {
            block_size: s.block_size,
            blocks: s.blocks as i64,
            free_blocks: s.free_blocks as i64,
            inodes: s.inodes as i64,
            free_inodes: s.free_inodes as i64,
            groups: s.groups as i64,
            messages,
        }),
        Err(e) => {
            let mut text = String::new();
            for m in messages.iter().filter(|m| m.level == "warning") {
                text.push_str(&format!("warning: {}\n", m.text));
            }
            text.push_str(&e.msg);
            Err(text)
        }
    }
}

fn check_output(output: &str) -> Result<PathBuf> {
    if output == "-" {
        return Err(err("writing the image to standard output is not supported here"));
    }
    Ok(PathBuf::from(output))
}

// Async tasks carry option errors so that they reject the promise instead of
// throwing synchronously.
pub struct BuildFileTask {
    job: Result<Job>,
}

impl Task for BuildFileTask {
    type Output = BuildResult;
    type JsValue = BuildResult;

    fn compute(&mut self) -> Result<Self::Output> {
        let job = self.job.as_ref().map_err(|e| err(&e.reason))?;
        run(job).map_err(err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct BuildBufferTask {
    job: Result<(Job, tempfile::TempDir)>,
}

impl Task for BuildBufferTask {
    type Output = Vec<u8>;
    type JsValue = Buffer;

    fn compute(&mut self) -> Result<Self::Output> {
        let (job, _dir) = self.job.as_ref().map_err(|e| err(&e.reason))?;
        run(job).map_err(err)?;
        std::fs::read(&job.options.image).map_err(err)
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.into())
    }
}

/// Build an image file. Runs on the libuv thread pool.
#[napi(ts_return_type = "Promise<BuildResult>")]
pub fn build(output: String, options: BuildOptions) -> AsyncTask<BuildFileTask> {
    AsyncTask::new(BuildFileTask { job: check_output(&output).and_then(|out| job(options, &out)) })
}

/// Build an image file, blocking the event loop.
#[napi]
pub fn build_sync(output: String, options: BuildOptions) -> Result<BuildResult> {
    let output = check_output(&output)?;
    run(&job(options, &output)?).map_err(err)
}

/// Build an image in memory. Runs on the libuv thread pool.
#[napi(ts_return_type = "Promise<Buffer>")]
pub fn build_buffer(options: BuildOptions) -> AsyncTask<BuildBufferTask> {
    let job = tempfile::tempdir().map_err(err).and_then(|dir| Ok((job(options, &dir.path().join("image.ext2"))?, dir)));
    AsyncTask::new(BuildBufferTask { job })
}

pub struct RunTask {
    args: Vec<String>,
}

impl Task for RunTask {
    type Output = RunResult;
    type JsValue = RunResult;

    fn compute(&mut self) -> Result<Self::Output> {
        let action = cli::parse(self.args.iter().map(OsString::from)).map_err(|e| err(format!("kext2: {e}")))?;
        let options = match action {
            Action::Help => {
                return Ok(RunResult { stdout: String::new(), stderr: cli::USAGE.to_string(), result: None })
            }
            Action::Version => {
                return Ok(RunResult {
                    stdout: format!("kext2 {}\n", cli::VERSION),
                    stderr: String::new(),
                    result: None,
                })
            }
            Action::Run(o) => *o,
        };
        check_output(&options.image.to_string_lossy())?;
        let stderr = Rc::new(RefCell::new(String::new()));
        let sink = stderr.clone();
        let r = with_messages(
            move |level, text| {
                let mut s = sink.borrow_mut();
                s.push_str(if level == Level::Warning { "kext2: warning: " } else { "kext2: " });
                s.push_str(text);
                s.push('\n');
            },
            || kext2::build(&options, std::env::var_os("SOURCE_DATE_EPOCH")),
        );
        let stderr = stderr.borrow().clone();
        match r {
            Ok(s) => Ok(RunResult {
                stdout: String::new(),
                stderr,
                result: Some(BuildResult {
                    block_size: s.block_size,
                    blocks: s.blocks as i64,
                    free_blocks: s.free_blocks as i64,
                    inodes: s.inodes as i64,
                    free_inodes: s.free_inodes as i64,
                    groups: s.groups as i64,
                    messages: Vec::new(),
                }),
            }),
            Err(e) => Err(err(format!("{stderr}kext2: {}", e.msg))),
        }
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// Run with command line arguments (without the program name), as the
/// `kext2` tool takes them, e.g.
/// `["--faketime", "--free-space", "64M", "--root", "dir", "out.ext2"]`.
/// Rejects with the error message when the tool would exit with an error.
#[napi(js_name = "kext2", ts_return_type = "Promise<RunResult>")]
pub fn kext2_cli(args: Vec<String>) -> AsyncTask<RunTask> {
    AsyncTask::new(RunTask { args })
}

/// Version of the kext2 library.
#[napi]
pub fn version() -> String {
    cli::VERSION.to_string()
}
