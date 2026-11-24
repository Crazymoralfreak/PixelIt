//! PixelIt worker: hosts ONNX Runtime in its own process.
//!
//! The application talks to the worker over stdin and stdout, so a crash of
//! a model or a driver ends this process and never the application. Every
//! message is a 4-byte little-endian length, a JSON header of that length and
//! then the tensor bytes the header announces.
//!
//! Requests: `init` (load the ONNX Runtime library), `load` (open a model),
//! `run` (run a model on tensors), `unload`, `ping`, `exit`.

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use ort::environment::Environment;
use ort::ep;
use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor, TensorElementType, ValueType};
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use windows::core::HSTRING;
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::SetDllDirectoryW;

/// Largest accepted JSON header.
const MAX_HEADER: usize = 1 << 20;

/// Largest accepted tensor payload of one message: 4 GiB.
const MAX_PAYLOAD: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Request {
    Init {
        runtime: String,
    },
    Load {
        model: String,
        path: String,
        #[serde(default)]
        device: Device,
        #[serde(default)]
        gpu: i32,
        #[serde(default)]
        threads: usize,
    },
    Run {
        model: String,
        inputs: Vec<TensorInfo>,
    },
    Unload {
        model: String,
    },
    Ping,
    Exit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Device {
    #[default]
    Gpu,
    Cpu,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct TensorInfo {
    name: String,
    /// `f32`, `u8` or `i64`.
    dtype: String,
    /// Dimensions; `-1` marks a dynamic one in a model description.
    shape: Vec<i64>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<Device>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<TensorInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    outputs: Vec<TensorInfo>,
}

impl Response {
    fn ok() -> Self {
        Self {
            ok: true,
            ..Self::default()
        }
    }

    fn failed(error: impl ToString) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
            ..Self::default()
        }
    }
}

#[derive(Default)]
struct Worker {
    environment: Option<Environment>,
    sessions: HashMap<String, Session>,
}

fn element_size(dtype: &str) -> Option<usize> {
    match dtype {
        "f32" => Some(4),
        "u8" => Some(1),
        "i64" => Some(8),
        _ => None,
    }
}

fn element_count(shape: &[i64]) -> Option<usize> {
    shape.iter().try_fold(1usize, |count, dimension| {
        usize::try_from(*dimension)
            .ok()
            .and_then(|dimension| count.checked_mul(dimension))
    })
}

fn payload_size(tensors: &[TensorInfo]) -> Option<u64> {
    tensors.iter().try_fold(0u64, |total, tensor| {
        let bytes = element_count(&tensor.shape)?.checked_mul(element_size(&tensor.dtype)?)?;
        total.checked_add(bytes as u64)
    })
}

fn describe(name: &str, dtype: &ValueType) -> TensorInfo {
    let kind = match dtype.tensor_type() {
        Some(TensorElementType::Float32) => "f32",
        Some(TensorElementType::Uint8) => "u8",
        Some(TensorElementType::Int64) => "i64",
        Some(TensorElementType::Float16) => "f16",
        _ => "other",
    };
    TensorInfo {
        name: name.to_owned(),
        dtype: kind.to_owned(),
        shape: dtype
            .tensor_shape()
            .map_or_default(|shape| shape.iter().copied().collect()),
    }
}

fn to_value(info: &TensorInfo, bytes: &[u8]) -> ort::Result<DynValue> {
    let shape = info.shape.clone();
    let value = match info.dtype.as_str() {
        "f32" => {
            let data: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();
            Tensor::from_array((shape, data))?.into_dyn()
        }
        "i64" => {
            let data: Vec<i64> = bytes
                .chunks_exact(8)
                .map(|chunk| {
                    let mut raw = [0u8; 8];
                    raw.copy_from_slice(chunk);
                    i64::from_le_bytes(raw)
                })
                .collect();
            Tensor::from_array((shape, data))?.into_dyn()
        }
        _ => Tensor::from_array((shape, bytes.to_vec()))?.into_dyn(),
    };
    Ok(value)
}

/// Reads an output tensor as 32-bit floats, whatever its element type.
fn output_floats(value: &DynValue) -> ort::Result<(Vec<i64>, Vec<f32>)> {
    if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
        return Ok((shape.iter().copied().collect(), data.to_vec()));
    }
    if let Ok((shape, data)) = value.try_extract_tensor::<u8>() {
        return Ok((
            shape.iter().copied().collect(),
            data.iter().map(|sample| f32::from(*sample)).collect(),
        ));
    }
    let (shape, data) = value.try_extract_tensor::<i64>()?;
    Ok((
        shape.iter().copied().collect(),
        data.iter().map(|sample| *sample as f32).collect(),
    ))
}

/// Makes Windows look for the libraries ONNX Runtime depends on (DirectML)
/// beside the runtime itself, ahead of the older copies in the system folder.
#[cfg(windows)]
fn use_library_folder(runtime: &str) {
    if let Some(folder) = Path::new(runtime).parent() {
        let folder = HSTRING::from(folder.as_os_str());
        // SAFETY: the argument is a valid NUL-terminated wide string that
        // lives until the call returns.
        if let Err(error) = unsafe { SetDllDirectoryW(&folder) } {
            eprintln!("pixelit-worker: cannot set the library folder: {error}");
        }
    }
}

#[cfg(not(windows))]
fn use_library_folder(runtime: &str) {
    let _ = Path::new(runtime);
}

impl Worker {
    fn init(&mut self, runtime: &str) -> Response {
        if self.environment.is_some() {
            return Response::ok();
        }
        use_library_folder(runtime);
        let built = ort::init_from(runtime)
            .map_err(|error| error.to_string())
            .and_then(|builder| {
                builder
                    .with_name("pixelit")
                    .with_telemetry(false)
                    .build()
                    .map_err(|error| error.to_string())
            });
        match built {
            Ok(environment) => {
                self.environment = Some(environment);
                Response::ok()
            }
            Err(error) => Response::failed(error),
        }
    }

    fn open(
        environment: &Environment,
        path: &str,
        device: Device,
        gpu: i32,
        threads: usize,
    ) -> ort::Result<Session> {
        let mut builder = Session::builder(environment)?;
        if threads > 0 {
            builder = builder.with_intra_threads(threads)?;
        }
        if device == Device::Gpu {
            // DirectML needs sequential execution without memory patterns.
            builder = builder
                .with_memory_pattern(false)?
                .with_parallel_execution(false)?
                .with_execution_providers([ep::DirectML::default()
                    .with_device_id(gpu)
                    .build()
                    .error_on_failure()])?;
        }
        builder.commit_from_file(path)
    }

    fn load(
        &mut self,
        model: &str,
        path: &str,
        device: Device,
        gpu: i32,
        threads: usize,
    ) -> Response {
        let Some(environment) = self.environment.as_ref() else {
            return Response::failed("runtime is not initialised");
        };
        // A model the video driver refuses still runs on the processor.
        let (session, used) = match Self::open(environment, path, device, gpu, threads) {
            Ok(session) => (session, device),
            Err(error) if device == Device::Gpu => {
                eprintln!("pixelit-worker: GPU session failed, using CPU: {error}");
                match Self::open(environment, path, Device::Cpu, gpu, threads) {
                    Ok(session) => (session, Device::Cpu),
                    Err(error) => return Response::failed(error),
                }
            }
            Err(error) => return Response::failed(error),
        };
        let response = Response {
            ok: true,
            device: Some(used),
            inputs: session
                .inputs()
                .iter()
                .map(|outlet| describe(outlet.name(), outlet.dtype()))
                .collect(),
            outputs: session
                .outputs()
                .iter()
                .map(|outlet| describe(outlet.name(), outlet.dtype()))
                .collect(),
            ..Response::default()
        };
        self.sessions.insert(model.to_owned(), session);
        response
    }

    fn run(&mut self, model: &str, inputs: &[TensorInfo], payload: &[u8]) -> (Response, Vec<u8>) {
        let Some(session) = self.sessions.get_mut(model) else {
            return (Response::failed("model is not loaded"), Vec::new());
        };
        let mut values: Vec<(String, SessionInputValue<'static>)> =
            Vec::with_capacity(inputs.len());
        let mut at = 0usize;
        for info in inputs {
            let size = match element_count(&info.shape).zip(element_size(&info.dtype)) {
                Some((count, size)) => count * size,
                None => return (Response::failed("bad input tensor"), Vec::new()),
            };
            let Some(bytes) = payload.get(at..at + size) else {
                return (Response::failed("input payload is too short"), Vec::new());
            };
            at += size;
            match to_value(info, bytes) {
                Ok(value) => values.push((info.name.clone(), value.into())),
                Err(error) => return (Response::failed(error), Vec::new()),
            }
        }
        let names: Vec<String> = session
            .outputs()
            .iter()
            .map(|outlet| outlet.name().to_owned())
            .collect();
        let outputs = match session.run(values) {
            Ok(outputs) => outputs,
            Err(error) => return (Response::failed(error), Vec::new()),
        };
        let mut response = Response::ok();
        let mut bytes = Vec::new();
        for name in names {
            let Some(value) = outputs.get(&name) else {
                continue;
            };
            match output_floats(value) {
                Ok((shape, data)) => {
                    bytes.reserve(data.len() * 4);
                    for sample in data {
                        bytes.extend_from_slice(&sample.to_le_bytes());
                    }
                    response.outputs.push(TensorInfo {
                        name,
                        dtype: String::from("f32"),
                        shape,
                    });
                }
                Err(error) => return (Response::failed(error), Vec::new()),
            }
        }
        (response, bytes)
    }
}

fn read_exact_or_end(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<bool> {
    match reader.read_exact(buffer) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn write_message(writer: &mut impl Write, response: &Response, payload: &[u8]) -> io::Result<()> {
    let header = serde_json::to_vec(response).map_err(io::Error::other)?;
    writer.write_all(&(header.len() as u32).to_le_bytes())?;
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    writer.flush()
}

fn serve(reader: &mut impl Read, writer: &mut impl Write) -> io::Result<()> {
    let mut worker = Worker::default();
    loop {
        let mut length = [0u8; 4];
        if !read_exact_or_end(reader, &mut length)? {
            return Ok(());
        }
        let length = u32::from_le_bytes(length) as usize;
        if length > MAX_HEADER {
            return Err(io::Error::other("header is too large"));
        }
        let mut header = vec![0u8; length];
        reader.read_exact(&mut header)?;
        let request: Request = match serde_json::from_slice(&header) {
            Ok(request) => request,
            Err(error) => {
                // The payload length is unknown, so the stream cannot continue.
                write_message(writer, &Response::failed(error), &[])?;
                return Err(io::Error::other("bad request header"));
            }
        };
        match request {
            Request::Init { runtime } => write_message(writer, &worker.init(&runtime), &[])?,
            Request::Load {
                model,
                path,
                device,
                gpu,
                threads,
            } => {
                let response = worker.load(&model, &path, device, gpu, threads);
                write_message(writer, &response, &[])?;
            }
            Request::Run { model, inputs } => {
                let Some(size) = payload_size(&inputs).filter(|size| *size <= MAX_PAYLOAD) else {
                    write_message(writer, &Response::failed("bad input size"), &[])?;
                    return Err(io::Error::other("bad input size"));
                };
                let mut payload = vec![0u8; size as usize];
                reader.read_exact(&mut payload)?;
                let (response, bytes) = worker.run(&model, &inputs, &payload);
                drop(payload);
                write_message(writer, &response, &bytes)?;
            }
            Request::Unload { model } => {
                worker.sessions.remove(&model);
                write_message(writer, &Response::ok(), &[])?;
            }
            Request::Ping => write_message(writer, &Response::ok(), &[])?,
            Request::Exit => {
                write_message(writer, &Response::ok(), &[])?;
                return Ok(());
            }
        }
    }
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = BufReader::with_capacity(1 << 20, stdin.lock());
    let mut writer = BufWriter::with_capacity(1 << 20, stdout.lock());
    if let Err(error) = serve(&mut reader, &mut writer) {
        eprintln!("pixelit-worker: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(header: &str, payload: &[u8]) -> Vec<u8> {
        let mut out = (header.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(header.as_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn responses(mut bytes: &[u8]) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        while bytes.len() >= 4 {
            let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
            out.push(serde_json::from_slice(&bytes[4..4 + length]).unwrap());
            bytes = &bytes[4 + length..];
        }
        out
    }

    #[test]
    fn answers_without_a_runtime() {
        let mut input = message(r#"{"op":"ping"}"#, &[]);
        input.extend(message(
            r#"{"op":"load","model":"m","path":"none.onnx"}"#,
            &[],
        ));
        input.extend(message(
            r#"{"op":"run","model":"m","inputs":[{"name":"x","dtype":"f32","shape":[1,2]}]}"#,
            &[0u8; 8],
        ));
        input.extend(message(r#"{"op":"exit"}"#, &[]));
        let mut output = Vec::new();
        serve(&mut input.as_slice(), &mut output).unwrap();
        let answers = responses(&output);
        assert_eq!(answers.len(), 4);
        assert_eq!(answers[0]["ok"], true);
        assert_eq!(answers[1]["ok"], false);
        assert_eq!(answers[2]["error"], "model is not loaded");
        assert_eq!(answers[3]["ok"], true);
    }

    #[test]
    fn sizes_are_checked() {
        let tensor = |dtype: &str, shape: &[i64]| TensorInfo {
            name: String::from("x"),
            dtype: dtype.to_owned(),
            shape: shape.to_vec(),
        };
        assert_eq!(payload_size(&[tensor("f32", &[1, 3, 4, 4])]), Some(192));
        assert_eq!(
            payload_size(&[tensor("u8", &[2, 2]), tensor("i64", &[1])]),
            Some(12)
        );
        assert_eq!(payload_size(&[tensor("f32", &[-1, 3])]), None);
        assert_eq!(payload_size(&[tensor("f64", &[1])]), None);
        assert_eq!(payload_size(&[tensor("f32", &[i64::MAX, i64::MAX])]), None);
    }

    #[test]
    fn end_of_input_stops_the_worker() {
        let mut output = Vec::new();
        let mut empty: &[u8] = &[];
        serve(&mut empty, &mut output).unwrap();
        assert_eq!(output, [] as [u8; 0]);
        let bad = message("not json", &[]);
        assert!(serve(&mut bad.as_slice(), &mut output).is_err());
    }
}
