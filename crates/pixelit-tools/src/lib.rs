//! One list of tools for every agent: the assistant chat inside the
//! application and external clients of the MCP server.
//!
//! Every tool obeys the agent permissions of the settings: files are read and
//! written only inside the allowed folders, existing files are replaced or
//! deleted only when the user switched that on, and nothing is downloaded or
//! sent to a cloud service without the matching permission.

pub mod args;
pub mod assistant;
mod files;
mod neural;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use pixelit_ai::{Ai, ToolSpec};
use pixelit_core::pipeline::{Conflict, Destination, Pipeline};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::batch::{self, BatchReport, BatchRequest};
use pixelit_engine::settings::AgentAccess;
use pixelit_engine::{scan, Engine, JobContext};

pub use crate::assistant::{Assistant, Builtin, Chat, ChatEvent, Chats, Model, ToolSource};

/// Most files listed in a tool answer; the counts stay exact.
pub const LISTED: usize = 200;

/// Everything a tool needs to run.
#[derive(Clone)]
pub struct ToolContext {
    pub engine: Arc<Engine>,
    pub ai: Arc<Ai>,
    pub cancel: CancelToken,
    /// Language for texts a model writes, as an interface language code.
    pub language: String,
}

/// A tool of the registry.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// JSON Schema of the arguments.
    pub schema: fn() -> Value,
    pub run: fn(&ToolContext, &Value) -> Result<Value>,
}

impl ToolContext {
    pub fn new(engine: Arc<Engine>, ai: Arc<Ai>) -> Self {
        let language = engine.settings().language;
        Self {
            engine,
            ai,
            cancel: CancelToken::new(),
            language: if language.is_empty() {
                String::from("en")
            } else {
                language
            },
        }
    }

    pub fn access(&self) -> AgentAccess {
        self.engine.settings().agents
    }

    /// Refuses a path outside the folders agents may use.
    pub fn check_path(&self, path: &Path) -> Result<()> {
        if self.access().allows(path) {
            Ok(())
        } else {
            Err(Error::new("agentPathDenied").with("path", path.display()))
        }
    }

    /// Refuses requests to servers outside this computer without permission.
    pub fn check_cloud(&self, leaves_computer: bool) -> Result<()> {
        if leaves_computer && !self.access().allow_cloud {
            Err(Error::new("agentNotAllowed").with("action", "cloud"))
        } else {
            Ok(())
        }
    }

    /// A job handle that follows the cancellation of this call.
    pub fn job(&self) -> JobContext {
        let mut job = JobContext::detached();
        job.cancel = self.cancel.clone();
        job
    }
}

/// Makes a pipeline safe for an agent: the destination must be allowed, and
/// replacing or deleting files needs the matching permission. Returns notes
/// about what was changed.
pub fn restrict(
    context: &ToolContext,
    pipeline: &mut Pipeline,
    destination: Option<&Path>,
) -> Result<Vec<&'static str>> {
    let access = context.access();
    let mut notes = Vec::new();
    if let Some(folder) = destination {
        context.check_path(folder)?;
        pipeline.output.destination = Destination::Folder {
            path: folder.to_string_lossy().into_owned(),
        };
    } else if let Destination::Folder { path } = &pipeline.output.destination {
        context.check_path(Path::new(path))?;
    }
    if pipeline.output.conflict == Conflict::Overwrite && !access.allow_overwrite {
        pipeline.output.conflict = Conflict::Rename;
        notes.push(
            "Replacing existing files is switched off for agents; new files were written instead.",
        );
    }
    if pipeline.output.replace_original && !access.allow_delete {
        pipeline.output.replace_original = false;
        notes.push("Deleting files is switched off for agents; the source files were kept.");
    }
    let repaints = pipeline
        .steps
        .iter()
        .any(|step| step.enabled && step.op.uses_generator());
    if repaints {
        context.check_cloud(context.ai.generation_leaves_computer())?;
    }
    Ok(notes)
}

/// Short form of a batch result for a model.
pub fn summarize(report: &BatchReport, notes: &[&str]) -> Value {
    let files: Vec<Value> = report
        .items
        .iter()
        .filter_map(|item| item.outcome.as_ref())
        .take(LISTED)
        .map(|outcome| {
            json!({
                "source": outcome.source,
                "output": outcome.output,
                "format": outcome.format.name(),
                "width": outcome.width,
                "height": outcome.height,
                "bytes": outcome.output_bytes,
            })
        })
        .collect();
    let errors: Vec<Value> = report
        .items
        .iter()
        .filter_map(|item| {
            item.error
                .as_ref()
                .map(|error| json!({ "source": item.source, "error": error.code, "details": error.params }))
        })
        .take(LISTED)
        .collect();
    json!({
        "done": report.done,
        "skipped": report.skipped,
        "failed": report.failed,
        "sourceBytes": report.source_bytes,
        "outputBytes": report.output_bytes,
        "folder": report.folder,
        "undoId": report.undo_id,
        "files": files,
        "errors": errors,
        "notes": notes,
    })
}

/// Runs a pipeline over files and folders on behalf of an agent.
pub fn run_pipeline(
    context: &ToolContext,
    title: &str,
    mut pipeline: Pipeline,
    inputs: &[PathBuf],
    destination: Option<&Path>,
    recursive: bool,
) -> Result<Value> {
    if inputs.is_empty() {
        return Err(Error::new("badArguments").with("name", "inputs"));
    }
    for input in inputs {
        context.check_path(input)?;
    }
    let notes = restrict(context, &mut pipeline, destination)?;
    // Results of an earlier run in the output subfolder are not sources.
    let skip = match &pipeline.output.destination {
        Destination::Subfolder { name } => Some(name.as_str()),
        Destination::SameFolder | Destination::Folder { .. } => None,
    };
    let items = scan::collect(inputs, recursive, skip);
    if items.is_empty() {
        return Err(Error::new("noImages"));
    }
    let settings = context.engine.settings();
    let request = BatchRequest {
        title: title.to_owned(),
        kind: String::from("agent"),
        items,
        pipeline,
        threads: settings.worker_threads(),
        memory_limit: u64::from(settings.memory_limit_mb) * 1024 * 1024,
    };
    let report = batch::run(
        &context.engine.runner(),
        Some(&context.engine.paths),
        &request,
        &context.job(),
    )?;
    Ok(summarize(&report, &notes))
}

/// Every tool, in the order they are offered.
pub fn tools() -> Vec<&'static Tool> {
    files::TOOLS.iter().chain(neural::TOOLS).collect()
}

/// The tools as a model sees them.
pub fn specs() -> Vec<ToolSpec> {
    tools()
        .into_iter()
        .map(|tool| ToolSpec {
            name: tool.name.to_owned(),
            description: tool.description.to_owned(),
            parameters: (tool.schema)(),
        })
        .collect()
}

/// Runs a tool by name.
pub fn call(context: &ToolContext, name: &str, arguments: &Value) -> Result<Value> {
    let tool = tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| Error::new("unknownTool").with("name", name))?;
    context.cancel.check()?;
    tracing::info!(tool = name, "agent tool");
    (tool.run)(context, arguments)
}

#[cfg(test)]
pub(crate) mod testing {
    use std::fs;

    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::{Color, Depth, Image, ImageFormat};
    use pixelit_engine::Paths;

    use super::*;

    /// A context over a temporary data folder with one allowed folder.
    pub fn context() -> (tempfile::TempDir, ToolContext, PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::open(Paths::at(folder.path().join("data"))).unwrap());
        let ai = Arc::new(Ai::open(engine.paths.clone()));
        // The application attaches the AI part to the engine when it starts.
        engine.set_ai(Some(ai.clone()));
        let allowed = folder.path().join("pictures");
        fs::create_dir_all(&allowed).unwrap();
        engine
            .update_settings(|settings| {
                settings.agents.folders = vec![allowed.to_string_lossy().into_owned()];
            })
            .unwrap();
        (folder, ToolContext::new(engine, ai), allowed)
    }

    pub fn picture(path: &Path, width: u32, height: u32) {
        let image = Image::new(width, height, Depth::Eight, Color::rgb(40, 120, 200)).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::write(path, bytes).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::testing::{context, picture};
    use super::*;

    #[test]
    fn tool_names_are_unique_and_schemas_are_objects() {
        let mut names = HashSet::new();
        for tool in tools() {
            assert!(names.insert(tool.name), "duplicate tool {}", tool.name);
            let schema = (tool.schema)();
            assert_eq!(schema["type"], "object", "{}", tool.name);
            assert!(schema["properties"].is_object(), "{}", tool.name);
            assert!(!tool.description.is_empty());
        }
        assert_eq!(specs().len(), names.len());
    }

    #[test]
    fn unknown_tools_are_reported() {
        let (_folder, context, _allowed) = context();
        let error = call(&context, "nope", &json!({})).unwrap_err();
        assert_eq!(error.code, "unknownTool");
    }

    #[test]
    fn pipelines_are_made_safe() {
        let (folder, context, allowed) = context();
        let mut pipeline = Pipeline::default();
        pipeline.output.conflict = Conflict::Overwrite;
        pipeline.output.replace_original = true;
        let notes = restrict(&context, &mut pipeline, Some(&allowed)).unwrap();
        assert_eq!(notes.len(), 2);
        assert_eq!(pipeline.output.conflict, Conflict::Rename);
        assert!(!pipeline.output.replace_original);

        let outside = folder.path().join("elsewhere");
        let error = restrict(&context, &mut pipeline, Some(&outside)).unwrap_err();
        assert_eq!(error.code, "agentPathDenied");

        context
            .engine
            .update_settings(|settings| {
                settings.agents.allow_overwrite = true;
                settings.agents.allow_delete = true;
            })
            .unwrap();
        let mut pipeline = Pipeline::default();
        pipeline.output.conflict = Conflict::Overwrite;
        pipeline.output.replace_original = true;
        assert!(restrict(&context, &mut pipeline, None).unwrap().is_empty());
        assert_eq!(pipeline.output.conflict, Conflict::Overwrite);
    }

    #[test]
    fn a_pipeline_runs_inside_the_allowed_folder() {
        let (folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 64, 48);
        let result = run_pipeline(
            &context,
            "test",
            Pipeline::default(),
            &[allowed.join("a.png")],
            None,
            false,
        )
        .unwrap();
        assert_eq!(result["done"], 1);
        assert_eq!(result["files"][0]["width"], 64);
        assert!(Path::new(result["files"][0]["output"].as_str().unwrap()).is_file());

        let outside = folder.path().join("b.png");
        picture(&outside, 8, 8);
        let error = run_pipeline(
            &context,
            "test",
            Pipeline::default(),
            &[outside],
            None,
            false,
        )
        .unwrap_err();
        assert_eq!(error.code, "agentPathDenied");
    }
}
