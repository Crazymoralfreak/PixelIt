//! Client of the worker process that hosts ONNX Runtime.
//!
//! Models run in `pixelit-worker`, a program of its own, so a crash inside a model or a video
//! driver ends the worker and the application reports an error and carries on.
//! Messages are a 4-byte little-endian length, a JSON header and tensor bytes.

use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::Deserialize;
use serde_json::{json, Value};

use pixelit_core::{Error, Result};

/// Keeps the worker from opening a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Largest accepted response header.
const MAX_HEADER: usize = 1 << 20;

/// Largest accepted tensor payload of a response: 4 GiB.
const MAX_PAYLOAD: u64 = 4 * 1024 * 1024 * 1024;

#[cfg(windows)]
const WORKER_NAME: &str = "pixelit-worker.exe";
#[cfg(not(windows))]
const WORKER_NAME: &str = "pixelit-worker";

/// A 32-bit float tensor.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tensor {
    pub name: String,
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

impl Tensor {
    pub fn new(name: &str, shape: &[usize], data: Vec<f32>) -> Self {
        Self {
            name: name.to_owned(),
            shape: shape.iter().map(|side| *side as i64).collect(),
            data,
        }
    }

    /// Side `index` of the shape, or zero.
    pub fn side(&self, index: usize) -> usize {
        self.shape
            .get(index)
            .and_then(|side| usize::try_from(*side).ok())
            .unwrap_or(0)
    }
}

/// An input or output of a model as the runtime describes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Port {
    pub name: String,
    pub dtype: String,
    /// `-1` marks a dimension the model leaves open.
    pub shape: Vec<i64>,
}

impl Port {
    /// A fixed side of the shape; `None` when the model leaves it open.
    pub fn fixed(&self, index: usize) -> Option<usize> {
        self.shape
            .get(index)
            .and_then(|side| usize::try_from(*side).ok())
            .filter(|side| *side > 0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelInfo {
    /// Whether the session runs on the video card.
    pub gpu: bool,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Header {
    ok: bool,
    error: Option<String>,
    device: Option<String>,
    inputs: Vec<Port>,
    outputs: Vec<Port>,
}

/// An open model and what is needed to open it again on the processor.
struct Loaded {
    info: ModelInfo,
    path: PathBuf,
    threads: usize,
}

pub struct Worker {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    loaded: HashMap<String, Loaded>,
    /// Models the video card could not run in this process.
    moved_to_cpu: Vec<String>,
}

/// Where the worker program is: beside the application.
pub fn worker_program() -> Result<PathBuf> {
    let beside = env::current_exe()
        .ok()
        .and_then(|program| program.parent().map(|folder| folder.join(WORKER_NAME)));
    match beside {
        Some(program) if program.is_file() => Ok(program),
        _ => Err(Error::new("aiWorkerMissing").with("file", WORKER_NAME)),
    }
}

fn broken(error: impl std::fmt::Display) -> Error {
    Error::new("aiWorkerFailed").detail(error)
}

/// The text of a refusal without what cannot be read: system messages of
/// Windows arrive in the local code page and turn into replacement marks.
fn readable(text: &str) -> String {
    text.chars()
        .filter(|sign| *sign != char::REPLACEMENT_CHARACTER && !sign.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn element_count(shape: &[i64]) -> Option<usize> {
    shape.iter().try_fold(1usize, |count, side| {
        usize::try_from(*side)
            .ok()
            .and_then(|side| count.checked_mul(side))
    })
}

impl Worker {
    /// Starts the worker and loads ONNX Runtime from `runtime`.
    /// Messages the worker prints go to `log`.
    pub fn start(program: &Path, runtime: &Path, log: Option<&Path>) -> Result<Self> {
        let errors = log
            .and_then(|path| {
                if let Some(folder) = path.parent() {
                    let _ = fs::create_dir_all(folder);
                }
                File::create(path).ok()
            })
            .map_or_else(Stdio::null, Stdio::from);
        let mut command = Command::new(program);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(errors);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = command.spawn().map_err(broken)?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err(broken("no pipes"));
        };
        let mut worker = Self {
            child,
            input: BufWriter::with_capacity(1 << 20, input),
            output: BufReader::with_capacity(1 << 20, output),
            loaded: HashMap::new(),
            moved_to_cpu: Vec::new(),
        };
        worker.call(&json!({ "op": "init", "runtime": runtime.to_string_lossy() }))?;
        Ok(worker)
    }

    fn send(&mut self, header: &Value, tensors: &[Tensor]) -> std::io::Result<()> {
        let header = header.to_string();
        self.input.write_all(&(header.len() as u32).to_le_bytes())?;
        self.input.write_all(header.as_bytes())?;
        for tensor in tensors {
            for sample in &tensor.data {
                self.input.write_all(&sample.to_le_bytes())?;
            }
        }
        self.input.flush()
    }

    /// Reads the JSON header of a response; a refusal becomes an error.
    fn read_header(&mut self) -> Result<Header> {
        let mut length = [0u8; 4];
        self.output.read_exact(&mut length).map_err(broken)?;
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_HEADER {
            return Err(broken("response header is too large"));
        }
        let mut raw = vec![0u8; length];
        self.output.read_exact(&mut raw).map_err(broken)?;
        let header: Header = serde_json::from_slice(&raw).map_err(broken)?;
        if header.ok {
            Ok(header)
        } else {
            Err(Error::new("aiModelFailed")
                .detail(readable(header.error.as_deref().unwrap_or_default())))
        }
    }

    /// Reads a response whose outputs are followed by their data.
    fn receive(&mut self) -> Result<Vec<Tensor>> {
        let header = self.read_header()?;
        let mut tensors = Vec::new();
        let mut total = 0u64;
        for port in &header.outputs {
            let Some(count) = element_count(&port.shape) else {
                return Err(broken("response has an open dimension"));
            };
            total += count as u64 * 4;
            if total > MAX_PAYLOAD {
                return Err(broken("response is too large"));
            }
            let mut bytes = vec![0u8; count * 4];
            self.output.read_exact(&mut bytes).map_err(broken)?;
            tensors.push(Tensor {
                name: port.name.clone(),
                shape: port.shape.clone(),
                data: bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect(),
            });
        }
        Ok(tensors)
    }

    /// Sends a request that is answered with a header only.
    fn call(&mut self, header: &Value) -> Result<Header> {
        self.send(header, &[]).map_err(broken)?;
        self.read_header()
    }

    /// Opens a model once; later calls return the stored description.
    pub fn load(
        &mut self,
        id: &str,
        path: &Path,
        gpu: bool,
        gpu_index: i32,
        threads: usize,
    ) -> Result<ModelInfo> {
        if let Some(model) = self.loaded.get(id) {
            return Ok(model.info.clone());
        }
        // The answer describes the model and carries no tensor data.
        let header = self.call(&json!({
            "op": "load",
            "model": id,
            "path": path.to_string_lossy(),
            "device": if gpu { "gpu" } else { "cpu" },
            "gpu": gpu_index,
            "threads": threads,
        }))?;
        let info = ModelInfo {
            gpu: header.device.as_deref() == Some("gpu"),
            inputs: header.inputs,
            outputs: header.outputs,
        };
        self.loaded.insert(
            id.to_owned(),
            Loaded {
                info: info.clone(),
                path: path.to_owned(),
                threads,
            },
        );
        Ok(info)
    }

    fn exchange(&mut self, id: &str, inputs: &[Tensor]) -> Result<Vec<Tensor>> {
        let described: Vec<Value> = inputs
            .iter()
            .map(|tensor| json!({ "name": tensor.name, "dtype": "f32", "shape": tensor.shape }))
            .collect();
        self.send(
            &json!({ "op": "run", "model": id, "inputs": described }),
            inputs,
        )
        .map_err(broken)?;
        self.receive()
    }

    /// Runs a loaded model. A model the video card opens and then fails to
    /// run is opened again on the processor and run there.
    pub fn run(&mut self, id: &str, inputs: &[Tensor]) -> Result<Vec<Tensor>> {
        for tensor in inputs {
            if element_count(&tensor.shape) != Some(tensor.data.len()) {
                return Err(Error::new("aiModelFailed").detail("tensor shape and data differ"));
            }
        }
        let refusal = match self.exchange(id, inputs) {
            Err(error) if error.code == "aiModelFailed" => error,
            other => return other,
        };
        let on_gpu = self.loaded.get(id).is_some_and(|model| model.info.gpu);
        if !on_gpu {
            return Err(refusal);
        }
        tracing::warn!(
            "{id} failed on the video card, running it on the processor: {}",
            refusal.params.get("detail").map_or("", String::as_str)
        );
        let Some(model) = self.loaded.remove(id) else {
            return Err(refusal);
        };
        let _ = self.call(&json!({ "op": "unload", "model": id }));
        self.moved_to_cpu.push(id.to_owned());
        self.load(id, &model.path, false, 0, model.threads)?;
        self.exchange(id, inputs)
    }

    /// Models that had to leave the video card since the last call.
    pub fn take_moved_to_cpu(&mut self) -> Vec<String> {
        std::mem::take(&mut self.moved_to_cpu)
    }

    /// Closes a model and frees its memory.
    pub fn unload(&mut self, id: &str) {
        if self.loaded.remove(id).is_some() {
            let _ = self.call(&json!({ "op": "unload", "model": id }));
        }
    }

    /// Ids of the open models.
    pub fn loaded(&self) -> Vec<String> {
        self.loaded.keys().cloned().collect()
    }

    /// Whether the process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.send(&json!({ "op": "exit" }), &[]);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensors_describe_their_sides() {
        let tensor = Tensor::new("x", &[1, 3, 4, 5], vec![0.0; 60]);
        assert_eq!(tensor.shape, [1, 3, 4, 5]);
        assert_eq!(tensor.side(3), 5);
        assert_eq!(tensor.side(9), 0);
        assert_eq!(element_count(&tensor.shape), Some(60));
    }

    #[test]
    fn open_dimensions_are_not_fixed() {
        let port = Port {
            name: String::from("input"),
            dtype: String::from("f32"),
            shape: vec![1, 3, -1, 128],
        };
        assert_eq!(port.fixed(2), None);
        assert_eq!(port.fixed(3), Some(128));
        assert_eq!(port.fixed(7), None);
        assert_eq!(element_count(&port.shape), None);
    }

    #[test]
    fn refusals_are_made_readable() {
        let raw = "MatMul failed\r\n  80070057 \u{fffd}\u{fffd}\u{fffd} \u{fffd}\u{fffd}.\n";
        assert_eq!(readable(raw), "MatMul failed 80070057 .");
        assert_eq!(readable("plain text"), "plain text");
    }

    #[test]
    fn a_missing_program_is_reported() {
        let error = Worker::start(Path::new("no-such-worker.exe"), Path::new("x.dll"), None)
            .err()
            .unwrap();
        assert_eq!(error.code, "aiWorkerFailed");
    }
}
