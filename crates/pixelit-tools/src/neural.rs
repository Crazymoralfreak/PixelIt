//! Tools that use neural models: background removal, enlargement, face
//! restoration, generation, questions about a picture, components.

use serde_json::{json, Value};

use pixelit_ai::generate::png;
use pixelit_ai::{vision, GenerateRequest, Source, VisionTask};
use pixelit_core::pipeline::{
    Conflict, Op, OutputFormat, Pipeline, RemoveBackground, RestoreFaces, Step, Upscale,
};
use pixelit_core::{naming, Color, Error, Result};
use pixelit_engine::fsops;

use crate::args::{self, boolean, choice, integer, object, path_list, string};
use crate::{run_pipeline, Tool, ToolContext};

/// Most pictures one generation call returns.
const MAX_COUNT: u64 = 4;

fn batch_properties(mut properties: Value) -> Value {
    properties["inputs"] = path_list("Absolute paths of image files or folders");
    properties["destination"] = string(
        "Absolute path of the output folder; by default a PixelIt subfolder beside each source",
    );
    properties
}

fn ai_step(
    context: &ToolContext,
    arguments: &Value,
    title: &str,
    op: Op,
    format: OutputFormat,
) -> Result<Value> {
    let mut pipeline = Pipeline::default();
    pipeline.steps.push(Step::new(title, op));
    pipeline.output.format = format;
    pipeline.output.naming = format!("{{name}}-{title}");
    run_pipeline(
        context,
        title,
        pipeline,
        &args::paths(arguments, "inputs")?,
        args::optional_path(arguments, "destination").as_deref(),
        false,
    )
}

fn ai_status_schema() -> Value {
    object(json!({}), &[])
}

// Every tool has the same signature: the registry stores them as function pointers.
#[allow(clippy::unnecessary_wraps)]
fn ai_status(context: &ToolContext, _arguments: &Value) -> Result<Value> {
    let ai = &context.ai;
    let components: Vec<Value> = ai
        .components()
        .iter()
        .map(|status| {
            json!({
                "id": status.id,
                "kind": status.kind,
                "name": status.name,
                "installed": status.installed,
                "megabytes": status.bytes / (1024 * 1024),
            })
        })
        .collect();
    let access = context.access();
    Ok(json!({
        "available": ai.capabilities(),
        "components": components,
        "hardware": ai.hardware(),
        "permissions": {
            "folders": access.folders,
            "overwrite": access.allow_overwrite,
            "delete": access.allow_delete,
            "downloads": access.allow_downloads,
            "cloud": access.allow_cloud,
        },
    }))
}

fn remove_background_schema() -> Value {
    object(
        batch_properties(json!({
            "background": string("Colour placed behind the subject as #rrggbb; transparent when omitted"),
        })),
        &["inputs"],
    )
}

fn remove_background(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let background = match args::optional_text(arguments, "background") {
        Some(text) => Some(
            Color::parse(&text)
                .ok_or_else(|| Error::new("badArguments").with("name", "background"))?,
        ),
        None => None,
    };
    ai_step(
        context,
        arguments,
        "cutout",
        Op::RemoveBackground(RemoveBackground {
            model: String::new(),
            background,
            keep: Vec::new(),
        }),
        OutputFormat::Png,
    )
}

fn upscale_schema() -> Value {
    object(
        batch_properties(json!({ "factor": integer("Enlargement: 2 or 4, default 2") })),
        &["inputs"],
    )
}

fn upscale(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let factor = args::whole(arguments, "factor").unwrap_or(2).clamp(2, 4) as u32;
    ai_step(
        context,
        arguments,
        "upscaled",
        Op::Upscale(Upscale {
            model: String::new(),
            factor,
        }),
        OutputFormat::Same,
    )
}

fn restore_faces_schema() -> Value {
    object(
        batch_properties(json!({
            "strength": integer("Blend of the restored face with the original, 0-100, default 80"),
        })),
        &["inputs"],
    )
}

fn restore_faces(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let strength = args::number(arguments, "strength")
        .unwrap_or(80.0)
        .clamp(0.0, 100.0) as f32;
    ai_step(
        context,
        arguments,
        "restored",
        Op::RestoreFaces(RestoreFaces {
            model: String::new(),
            strength,
        }),
        OutputFormat::Same,
    )
}

fn generate_schema() -> Value {
    object(
        json!({
            "prompt": string("What to draw"),
            "folder": string("Absolute path of the folder for the results"),
            "negative": string("What to avoid"),
            "source": string("Absolute path of a picture to change following the prompt"),
            "strength": { "type": "number", "description": "How far the result may move from the source, 0-1, default 0.6" },
            "width": integer("Width in pixels; the settings decide when omitted"),
            "height": integer("Height in pixels; the settings decide when omitted"),
            "count": integer("Number of pictures, 1-4"),
            "seed": integer("Seed for a repeatable result"),
        }),
        &["prompt", "folder"],
    )
}

fn generate_image(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let prompt = args::text(arguments, "prompt")?;
    let folder = args::path(arguments, "folder")?;
    context.check_path(&folder)?;
    context.check_cloud(context.ai.generation_leaves_computer())?;
    let side = |name: &str| args::whole(arguments, name).unwrap_or(0).min(8192) as u32;
    let request = GenerateRequest {
        prompt: prompt.clone(),
        negative: args::optional_text(arguments, "negative").unwrap_or_default(),
        width: side("width"),
        height: side("height"),
        seed: args::whole(arguments, "seed"),
        count: args::whole(arguments, "count")
            .unwrap_or(1)
            .clamp(1, MAX_COUNT) as u32,
        strength: args::number(arguments, "strength")
            .unwrap_or(0.6)
            .clamp(0.0, 1.0) as f32,
        ..GenerateRequest::default()
    };
    let source = match args::optional_path(arguments, "source") {
        Some(path) => {
            context.check_path(&path)?;
            let decoded = context.engine.runner().load(&path, 0, &context.cancel)?;
            Some(
                decoded
                    .media
                    .into_first()
                    .ok_or_else(|| Error::new("badImage").with("path", path.display()))?,
            )
        }
        None => None,
    };
    let source = source.as_ref().map(|image| Source { image, mask: None });
    let images = context
        .ai
        .generate(&request, source.as_ref(), &context.cancel)?;
    let stem: String = naming::sanitize(&prompt).chars().take(48).collect();
    let mut files = Vec::with_capacity(images.len());
    for image in &images {
        let written = fsops::write_file(
            &folder.join(format!("{}.png", stem.trim())),
            &png(image)?,
            Conflict::Rename,
        )?;
        files.push(json!({
            "path": written.path(),
            "width": image.width(),
            "height": image.height(),
        }));
    }
    Ok(json!({ "files": files }))
}

const TASKS: [&str; 5] = ["describe", "altText", "tags", "name", "ocr"];

fn describe_schema() -> Value {
    object(
        json!({
            "path": string("Absolute path of the image"),
            "task": choice(
                "describe: a short description; altText: alt text for a web page; tags: keywords; name: a file name; ocr: the text in the picture",
                &TASKS,
            ),
            "language": string("Language code of the answer such as en or ru; the interface language by default"),
            "apply": boolean("Save the answer: describe, altText and tags are written into the file metadata (JPEG, PNG), name renames the file. Needs the permission to replace files"),
        }),
        &["path"],
    )
}

fn describe_image(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let path = args::path(arguments, "path")?;
    context.check_path(&path)?;
    let task: VisionTask = match args::optional_text(arguments, "task") {
        Some(name) => serde_json::from_value(Value::String(name))
            .map_err(|_| Error::new("badArguments").with("name", "task"))?,
        None => VisionTask::Describe,
    };
    let apply = args::flag(arguments, "apply") && task != VisionTask::Ocr;
    if apply && !context.access().allow_overwrite {
        return Err(Error::new("agentNotAllowed").with("action", "overwrite"));
    }
    context.check_cloud(vision::leaves_computer(&context.ai.settings()))?;
    let language = args::optional_text(arguments, "language").unwrap_or(context.language.clone());
    let image = context
        .engine
        .runner()
        .load(&path, 0, &context.cancel)?
        .media
        .into_first()
        .ok_or_else(|| Error::new("badImage").with("path", path.display()))?;
    let text = context.ai.vision(&image, task, &language)?;
    let mut answer = json!({ "path": path, "text": text });
    if apply {
        match task {
            VisionTask::Describe | VisionTask::AltText => {
                vision::write_caption(&path, &text, &[])?;
            }
            VisionTask::Tags => {
                vision::write_caption(&path, "", &vision::tags(&text))?;
            }
            VisionTask::Name => {
                answer["path"] = json!(vision::rename(&path, &text)?);
            }
            VisionTask::Ocr => {}
        }
        answer["applied"] = json!(true);
    }
    Ok(answer)
}

fn install_schema() -> Value {
    object(
        json!({ "id": string("Component or model id from ai_status") }),
        &["id"],
    )
}

fn install_component(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let id = args::text(arguments, "id")?;
    if !context.access().allow_downloads {
        return Err(Error::new("agentNotAllowed").with("action", "downloads"));
    }
    context
        .ai
        .install(&id, &mut |_done, _total| {}, &context.cancel)?;
    Ok(json!({ "installed": id }))
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "ai_status",
        description: "Which AI features are ready, which models and components are installed, the hardware and what agents are allowed to do.",
        schema: ai_status_schema,
        run: ai_status,
    },
    Tool {
        name: "remove_background",
        description: "Cut the subject out of pictures with a local model. Writes PNG files with transparency or with a solid background colour.",
        schema: remove_background_schema,
        run: remove_background,
    },
    Tool {
        name: "upscale",
        description: "Enlarge pictures 2 or 4 times with a local super-resolution model.",
        schema: upscale_schema,
        run: upscale,
    },
    Tool {
        name: "restore_faces",
        description: "Restore blurry or damaged faces in photos with a local model.",
        schema: restore_faces_schema,
        run: restore_faces,
    },
    Tool {
        name: "generate_image",
        description: "Generate pictures from a text prompt, or change an existing picture following a prompt, with the generation provider of the settings.",
        schema: generate_schema,
        run: generate_image,
    },
    Tool {
        name: "describe_image",
        description: "Ask the vision model about a picture: description, alt text, keywords, a file name or the text in it. Can save the answer into the file.",
        schema: describe_schema,
        run: describe_image,
    },
    Tool {
        name: "install_component",
        description: "Download and install an AI model or component from the built-in catalogue. Needs the permission to download.",
        schema: install_schema,
        run: install_component,
    },
];

#[cfg(test)]
mod tests {
    use crate::call;
    use crate::testing::{context, picture};

    use super::*;

    #[test]
    fn status_lists_components_and_permissions() {
        let (_folder, context, _allowed) = context();
        let status = call(&context, "ai_status", &json!({})).unwrap();
        assert_eq!(status["available"]["background"], false);
        assert!(!status["components"].as_array().unwrap().is_empty());
        assert_eq!(status["permissions"]["downloads"], false);
        assert_eq!(
            status["permissions"]["folders"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn missing_models_are_reported_per_file() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 32, 32);
        let result = call(
            &context,
            "remove_background",
            &json!({ "inputs": [allowed.join("a.png")] }),
        )
        .unwrap();
        assert_eq!(result["failed"], 1);
        assert_eq!(result["errors"][0]["error"], "modelMissing");
        let bad = call(
            &context,
            "remove_background",
            &json!({ "inputs": [allowed.join("a.png")], "background": "greenish" }),
        )
        .unwrap_err();
        assert_eq!(bad.code, "badArguments");
    }

    #[test]
    fn downloads_and_saving_need_permission() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 32, 32);
        let install = call(&context, "install_component", &json!({ "id": "u2netp" })).unwrap_err();
        assert_eq!(install.code, "agentNotAllowed");
        assert_eq!(install.params["action"], "downloads");
        let apply = call(
            &context,
            "describe_image",
            &json!({ "path": allowed.join("a.png"), "apply": true }),
        )
        .unwrap_err();
        assert_eq!(apply.params["action"], "overwrite");
        let unset = call(
            &context,
            "describe_image",
            &json!({ "path": allowed.join("a.png"), "task": "tags" }),
        )
        .unwrap_err();
        assert_eq!(unset.code, "providerNotSet");
    }

    #[test]
    fn generation_needs_a_provider_and_an_allowed_folder() {
        let (folder, context, allowed) = context();
        let unset = call(
            &context,
            "generate_image",
            &json!({ "prompt": "a fox", "folder": allowed }),
        )
        .unwrap_err();
        assert_eq!(unset.code, "providerNotSet");
        let denied = call(
            &context,
            "generate_image",
            &json!({ "prompt": "a fox", "folder": folder.path().join("other") }),
        )
        .unwrap_err();
        assert_eq!(denied.code, "agentPathDenied");
    }
}
