//! Tools that work on files without neural models: listing, information,
//! conversion, recipes, pipelines, comparison, duplicates and undo.

use std::fs;

use serde_json::{json, Value};

use pixelit_codecs::probe_file;
use pixelit_core::pipeline::{
    MetadataMode, Op, OutputFormat, OutputSpec, Pipeline, Resize, ResizeMode, Step, Target,
};
use pixelit_core::{Error, ImageFormat, Result};
use pixelit_engine::fsops::UndoLog;
use pixelit_engine::journal::Journal;
use pixelit_engine::recipes::Recipes;
use pixelit_engine::{compress, duplicates, scan};

use crate::args::{self, boolean, choice, integer, object, path_list, string};
use crate::{run_pipeline, Tool, ToolContext, LISTED};

const FORMATS: [&str; 10] = [
    "same", "auto", "jpeg", "png", "webp", "avif", "jxl", "tiff", "gif", "bmp",
];

const PIPELINE_HELP: &str = "Run a custom processing pipeline over images. `pipeline` is an object {steps:[{op:{type,...}}], output:{...}}. \
Step types and their fields: \
resize{mode:fit|fill|exact|percent|longSide|shortSide|width|height,width,height,percent,upscale}; \
crop{mode:rect|aspect,x,y,width,height (fractions 0..1),aspectWidth,aspectHeight,anchor}; \
rotate{angle,expand}; flip{horizontal,vertical}; \
adjust{exposure,brightness,contrast,highlights,shadows,saturation,vibrance,temperature,tint (each -100..100)}; \
filter{kind:sharpen|blur|denoise|grain|vignette|grayscale|sepia|invert,amount,radius}; \
watermark{kind:text|image,text,image,size,scale,anchor,margin,opacity,tile}; \
removeBackground{background:'#rrggbb' or null}; upscale{factor:2|4}; restoreFaces{strength:0..100}. \
Output fields: format (same|auto|jpeg|png|webp|avif|jxl|tiff|gif|bmp), quality 1..100, lossless, \
target {kind:'bytes',bytes} or {kind:'quality',score}, metadata {mode:keep|strip}, \
naming (template such as '{name}-{width}'), conflict (rename|skip). Omitted fields take defaults.";

fn bad(name: &str) -> Error {
    Error::new("badArguments").with("name", name)
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|error| Error::new("internal").detail(error))
}

fn output_format(name: &str) -> Result<OutputFormat> {
    serde_json::from_value(Value::String(name.to_ascii_lowercase())).map_err(|_| bad("format"))
}

fn list_images_schema() -> Value {
    object(
        json!({
            "folder": string("Absolute path of the folder"),
            "recursive": boolean("Include subfolders"),
        }),
        &["folder"],
    )
}

fn list_images(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let folder = args::path(arguments, "folder")?;
    context.check_path(&folder)?;
    let files = scan::folder_files(&folder, args::flag(arguments, "recursive"), None);
    let listed: Vec<Value> = files
        .iter()
        .take(LISTED)
        .map(|path| {
            json!({
                "path": path,
                "bytes": fs::metadata(path).map_or(0, |meta| meta.len()),
            })
        })
        .collect();
    Ok(json!({
        "count": files.len(),
        "truncated": files.len() > LISTED,
        "files": listed,
    }))
}

fn image_info_schema() -> Value {
    object(
        json!({ "path": string("Absolute path of the image") }),
        &["path"],
    )
}

fn image_info(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let path = args::path(arguments, "path")?;
    context.check_path(&path)?;
    let mut info = to_json(&probe_file(&path)?)?;
    info["path"] = json!(path);
    info["bytes"] = json!(fs::metadata(&path)?.len());
    Ok(info)
}

fn no_arguments() -> Value {
    object(json!({}), &[])
}

// Every tool has the same signature: the registry stores them as function pointers.
#[allow(clippy::unnecessary_wraps)]
fn list_recipes(context: &ToolContext, _arguments: &Value) -> Result<Value> {
    let recipes: Vec<Value> = Recipes::load(&context.engine.paths)
        .all()
        .iter()
        .map(|recipe| {
            let steps: Vec<&str> = recipe
                .pipeline
                .steps
                .iter()
                .filter(|step| step.enabled)
                .map(|step| step.op.kind())
                .collect();
            json!({
                "id": recipe.id,
                "name": recipe.name,
                "description": recipe.description,
                "steps": steps,
                "output": recipe.pipeline.output.format,
            })
        })
        .collect();
    Ok(json!({ "recipes": recipes }))
}

fn batch_properties(mut properties: Value) -> Value {
    properties["inputs"] = path_list("Absolute paths of image files or folders");
    properties["destination"] = string(
        "Absolute path of the output folder; by default a PixelIt subfolder beside each source",
    );
    properties["recursive"] = boolean("Include subfolders of input folders");
    properties
}

fn run_recipe_schema() -> Value {
    object(
        batch_properties(json!({ "recipe": string("Recipe id or name, see list_recipes") })),
        &["recipe", "inputs"],
    )
}

fn run_recipe(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let key = args::text(arguments, "recipe")?;
    let recipe = Recipes::load(&context.engine.paths)
        .find(&key)
        .ok_or_else(|| Error::new("recipeNotFound").with("name", &key))?;
    // Built-in recipes get their names in the window; here the id serves.
    let title = if recipe.name.is_empty() {
        recipe.id
    } else {
        recipe.name
    };
    run_pipeline(
        context,
        &title,
        recipe.pipeline,
        &args::paths(arguments, "inputs")?,
        args::optional_path(arguments, "destination").as_deref(),
        args::flag(arguments, "recursive"),
    )
}

fn convert_schema() -> Value {
    object(
        batch_properties(json!({
            "format": choice("Output format; `same` keeps the source format, `auto` picks the smallest", &FORMATS),
            "quality": integer("Quality 1-100 for lossy formats, default 82"),
            "lossless": boolean("Lossless compression where the format has it"),
            "maxWidth": integer("Shrink to fit this width in pixels"),
            "maxHeight": integer("Shrink to fit this height in pixels"),
            "targetKb": integer("Make every file fit into this many kilobytes"),
            "stripMetadata": boolean("Remove EXIF, GPS and other metadata"),
        })),
        &["inputs"],
    )
}

/// Builds the pipeline of the `convert` tool.
fn convert_pipeline(arguments: &Value) -> Result<Pipeline> {
    let mut output = OutputSpec::default();
    if let Some(format) = args::optional_text(arguments, "format") {
        output.format = output_format(&format)?;
    }
    if let Some(quality) = args::whole(arguments, "quality") {
        output.quality = quality.clamp(1, 100) as u8;
    }
    output.lossless = args::flag(arguments, "lossless");
    if let Some(kilobytes) = args::whole(arguments, "targetKb").filter(|value| *value > 0) {
        output.target = Target::Bytes {
            bytes: kilobytes.saturating_mul(1024),
        };
    }
    if args::flag(arguments, "stripMetadata") {
        output.metadata.mode = MetadataMode::Strip;
    }
    let width = args::whole(arguments, "maxWidth").unwrap_or(0).min(65_535) as u32;
    let height = args::whole(arguments, "maxHeight").unwrap_or(0).min(65_535) as u32;
    let mut steps = Vec::new();
    if width > 0 || height > 0 {
        steps.push(Step::new(
            "resize",
            Op::Resize(Resize {
                mode: ResizeMode::Fit,
                width,
                height,
                ..Resize::default()
            }),
        ));
    }
    Ok(Pipeline {
        steps,
        output,
        ..Pipeline::default()
    })
}

fn convert(context: &ToolContext, arguments: &Value) -> Result<Value> {
    run_pipeline(
        context,
        "convert",
        convert_pipeline(arguments)?,
        &args::paths(arguments, "inputs")?,
        args::optional_path(arguments, "destination").as_deref(),
        args::flag(arguments, "recursive"),
    )
}

fn run_pipeline_schema() -> Value {
    object(
        batch_properties(json!({
            "pipeline": { "type": "object", "description": "Steps and output settings" },
        })),
        &["pipeline", "inputs"],
    )
}

fn custom_pipeline(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let pipeline: Pipeline = match &arguments["pipeline"] {
        Value::Object(_) => serde_json::from_value(arguments["pipeline"].clone()),
        // Some models send nested objects as JSON text.
        Value::String(text) => serde_json::from_str(text),
        _ => return Err(bad("pipeline")),
    }
    .map_err(|error| bad("pipeline").detail(error))?;
    run_pipeline(
        context,
        "pipeline",
        pipeline,
        &args::paths(arguments, "inputs")?,
        args::optional_path(arguments, "destination").as_deref(),
        args::flag(arguments, "recursive"),
    )
}

fn compare_formats_schema() -> Value {
    object(
        json!({
            "path": string("Absolute path of the image"),
            "formats": path_list("Formats to compare: jpeg, png, webp, avif, jxl; all five by default"),
            "quality": integer("Quality 1-100, default 82"),
        }),
        &["path"],
    )
}

fn compare_formats(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let path = args::path(arguments, "path")?;
    context.check_path(&path)?;
    let mut formats: Vec<ImageFormat> = arguments["formats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(ImageFormat::from_name)
        .filter(|format| format.can_encode())
        .collect();
    if formats.is_empty() {
        formats = vec![
            ImageFormat::Jpeg,
            ImageFormat::Png,
            ImageFormat::Webp,
            ImageFormat::Avif,
            ImageFormat::Jxl,
        ];
    }
    let mut spec = OutputSpec::default();
    if let Some(quality) = args::whole(arguments, "quality") {
        spec.quality = quality.clamp(1, 100) as u8;
    }
    let decoded = context.engine.runner().load(&path, 0, &context.cancel)?;
    let rows: Vec<Value> = compress::compare_formats(
        &decoded.media,
        &spec,
        &formats,
        &decoded.metadata,
        &context.cancel,
    )
    .iter()
    .map(|row| {
        json!({
            "format": row.format.name(),
            "bytes": row.bytes,
            "score": row.score,
            "error": row.error.as_ref().map(|error| error.code.clone()),
        })
    })
    .collect();
    context.cancel.check()?;
    Ok(json!({
        "sourceBytes": fs::metadata(&path)?.len(),
        "quality": spec.quality,
        "results": rows,
    }))
}

fn find_duplicates_schema() -> Value {
    object(
        json!({
            "folder": string("Absolute path of the folder"),
            "recursive": boolean("Include subfolders"),
            "tolerance": integer("0 finds pictures that look identical, up to 16 finds similar ones; default 6"),
        }),
        &["folder"],
    )
}

fn find_duplicates(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let folder = args::path(arguments, "folder")?;
    context.check_path(&folder)?;
    let files = scan::folder_files(&folder, args::flag(arguments, "recursive"), None);
    let tolerance = args::whole(arguments, "tolerance")
        .unwrap_or(6)
        .min(u64::from(duplicates::MAX_DISTANCE)) as u32;
    let groups = duplicates::find(&context.engine.runner(), &files, tolerance, &context.job())?;
    Ok(json!({
        "scanned": files.len(),
        "groups": groups.len(),
        "truncated": groups.len() > LISTED,
        "duplicates": to_json(&groups.iter().take(LISTED).collect::<Vec<_>>())?,
    }))
}

fn undo_schema() -> Value {
    object(
        json!({ "undoId": string("`undoId` of a finished run; the most recent run by default") }),
        &[],
    )
}

fn undo(context: &ToolContext, arguments: &Value) -> Result<Value> {
    let paths = &context.engine.paths;
    let log = UndoLog::load(paths);
    let record = match args::optional_text(arguments, "undoId") {
        Some(id) => log.records.iter().find(|record| record.id == id),
        None => log.records.last(),
    }
    .ok_or_else(|| Error::new("undoNotFound"))?;
    // A run started by the user elsewhere stays out of reach of agents.
    for path in record
        .created
        .iter()
        .map(|file| &file.path)
        .chain(&record.trashed)
    {
        context.check_path(path)?;
    }
    let id = record.id.clone();
    let report = UndoLog::undo(paths, &id)?;
    Journal::forget_undo(paths, &id)?;
    to_json(&report)
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "list_images",
        description: "List the image files of a folder with their sizes.",
        schema: list_images_schema,
        run: list_images,
    },
    Tool {
        name: "image_info",
        description: "Format, dimensions, bit depth, transparency, frame count and the kinds of metadata an image file carries.",
        schema: image_info_schema,
        run: image_info,
    },
    Tool {
        name: "list_recipes",
        description: "List the saved processing recipes with their steps and output format.",
        schema: no_arguments,
        run: list_recipes,
    },
    Tool {
        name: "run_recipe",
        description: "Process image files or folders with a saved recipe. New files are written; sources stay untouched.",
        schema: run_recipe_schema,
        run: run_recipe,
    },
    Tool {
        name: "convert",
        description: "Convert, compress or shrink images: choose the format, quality, largest size and an optional file size limit.",
        schema: convert_schema,
        run: convert,
    },
    Tool {
        name: "run_pipeline",
        description: PIPELINE_HELP,
        schema: run_pipeline_schema,
        run: custom_pipeline,
    },
    Tool {
        name: "compare_formats",
        description: "Encode one image into several formats at the same quality and report the size and visual score of each. Writes nothing.",
        schema: compare_formats_schema,
        run: compare_formats,
    },
    Tool {
        name: "find_duplicates",
        description: "Find identical and visually similar pictures in a folder. Reports groups; deletes nothing.",
        schema: find_duplicates_schema,
        run: find_duplicates,
    },
    Tool {
        name: "undo",
        description: "Undo a finished run: its new files go to the Recycle Bin and replaced originals come back.",
        schema: undo_schema,
        run: undo,
    },
];

#[cfg(test)]
mod tests {
    use crate::call;
    use crate::testing::{context, picture};

    use super::*;

    #[test]
    fn folders_are_listed_and_described() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 20, 10);
        picture(&allowed.join("b.png"), 30, 30);
        let listed = call(&context, "list_images", &json!({ "folder": allowed })).unwrap();
        assert_eq!(listed["count"], 2);
        let info = call(
            &context,
            "image_info",
            &json!({ "path": allowed.join("a.png") }),
        )
        .unwrap();
        assert_eq!(info["width"], 20);
        assert_eq!(info["height"], 10);
        assert!(info["bytes"].as_u64().unwrap() > 0);
        let denied = call(&context, "list_images", &json!({ "folder": "C:\\" })).unwrap_err();
        assert_eq!(denied.code, "agentPathDenied");
    }

    #[test]
    fn convert_builds_the_expected_pipeline() {
        let pipeline = convert_pipeline(&json!({
            "format": "WebP",
            "quality": 300,
            "maxWidth": 800,
            "targetKb": 200,
            "stripMetadata": true,
        }))
        .unwrap();
        assert_eq!(pipeline.output.format, OutputFormat::Webp);
        assert_eq!(pipeline.output.quality, 100);
        assert_eq!(pipeline.output.target, Target::Bytes { bytes: 204_800 });
        assert_eq!(pipeline.output.metadata.mode, MetadataMode::Strip);
        match &pipeline.steps[0].op {
            Op::Resize(resize) => {
                assert_eq!((resize.width, resize.height), (800, 0));
                assert_eq!(resize.mode, ResizeMode::Fit);
            }
            other => panic!("unexpected step {other:?}"),
        }
        assert!(convert_pipeline(&json!({})).unwrap().steps.is_empty());
        assert_eq!(
            convert_pipeline(&json!({ "format": "exe" }))
                .unwrap_err()
                .code,
            "badArguments"
        );
    }

    #[test]
    fn images_are_converted() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 400, 200);
        let result = call(
            &context,
            "convert",
            &json!({ "inputs": [allowed.join("a.png")], "format": "jpeg", "maxWidth": 100 }),
        )
        .unwrap();
        assert_eq!(result["done"], 1);
        assert_eq!(result["files"][0]["format"], "jpeg");
        assert_eq!(result["files"][0]["width"], 100);
        assert_eq!(result["files"][0]["height"], 50);
    }

    #[test]
    fn recipes_are_listed_and_run() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 64, 64);
        let listed = call(&context, "list_recipes", &json!({})).unwrap();
        let first = listed["recipes"][0]["id"].as_str().unwrap().to_owned();
        let result = call(
            &context,
            "run_recipe",
            &json!({ "recipe": first, "inputs": allowed.join("a.png") }),
        )
        .unwrap();
        assert_eq!(result["done"], 1);
        let missing = call(
            &context,
            "run_recipe",
            &json!({ "recipe": "no such recipe", "inputs": [allowed.join("a.png")] }),
        )
        .unwrap_err();
        assert_eq!(missing.code, "recipeNotFound");
    }

    #[test]
    fn custom_pipelines_accept_objects_and_text() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 64, 32);
        let pipeline = json!({
            "steps": [{ "op": { "type": "flip", "horizontal": true } }],
            "output": { "format": "png" },
        });
        for form in [pipeline.clone(), Value::String(pipeline.to_string())] {
            let result = call(
                &context,
                "run_pipeline",
                &json!({ "pipeline": form, "inputs": [allowed.join("a.png")] }),
            )
            .unwrap();
            assert_eq!(result["done"], 1);
        }
        let error = call(
            &context,
            "run_pipeline",
            &json!({ "pipeline": { "steps": [{ "op": { "type": "explode" } }] }, "inputs": ["x"] }),
        )
        .unwrap_err();
        assert_eq!(error.code, "badArguments");
    }

    #[test]
    fn formats_are_compared_without_writing() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 64, 64);
        let result = call(
            &context,
            "compare_formats",
            &json!({ "path": allowed.join("a.png"), "formats": ["jpeg", "png"] }),
        )
        .unwrap();
        assert_eq!(result["results"].as_array().unwrap().len(), 2);
        assert_eq!(fs::read_dir(&allowed).unwrap().count(), 1);
    }

    #[test]
    fn duplicates_are_found() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 64, 64);
        picture(&allowed.join("b.png"), 64, 64);
        let result = call(&context, "find_duplicates", &json!({ "folder": allowed })).unwrap();
        assert_eq!(result["scanned"], 2);
        assert_eq!(result["groups"], 1);
    }

    #[test]
    fn undo_without_runs_is_reported() {
        let (_folder, context, _allowed) = context();
        let error = call(&context, "undo", &json!({})).unwrap_err();
        assert_eq!(error.code, "undoNotFound");
    }
}
