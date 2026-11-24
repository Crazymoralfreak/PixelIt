//! The command line of PixelIt, built into the same `PixelIt.exe`:
//!
//! ```text
//! PixelIt convert photos -f webp -q 80 --max-width 1920
//! PixelIt run "Web 1600" photos -o out
//! PixelIt mcp
//! ```
//!
//! The commands use the same engine, recipes and settings as the window.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use pixelit_ai::Ai;
use pixelit_codecs::probe_file;
use pixelit_core::pipeline::{
    Conflict, Destination, MetadataMode, Op, OutputFormat, Pipeline, Resize, ResizeMode, Step,
    Target,
};
use pixelit_core::{CancelToken, Error, ImageFormat, Result};
use pixelit_engine::batch::{self, BatchReport, BatchRequest};
use pixelit_engine::recipes::{self, Recipes};
use pixelit_engine::{compress, duplicates, scan, Engine, JobContext};
use pixelit_tools::ToolContext;

/// Everything went well.
pub const EXIT_OK: i32 = 0;
/// At least one file failed.
pub const EXIT_FAILED: i32 = 1;
/// The command line could not be understood.
pub const EXIT_USAGE: i32 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "PixelIt",
    version,
    about = "PixelIt: convert, compress and edit images in batches",
    after_help = "Run PixelIt without a command to open the window."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Convert, compress or shrink images
    Convert(ConvertArgs),
    /// Process images with a saved recipe
    Run(RunArgs),
    /// List the saved recipes
    Recipes {
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Show format, size and metadata of image files
    Info {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Encode one image into several formats and compare the sizes
    Compare {
        file: PathBuf,
        /// Formats to compare
        #[arg(short, long, value_enum, value_delimiter = ',', default_values_t = [Format::Jpeg, Format::Png, Format::Webp, Format::Avif, Format::Jxl])]
        formats: Vec<Format>,
        /// Quality from 1 to 100
        #[arg(short, long, default_value_t = 82, value_parser = clap::value_parser!(u8).range(1..=100))]
        quality: u8,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Find identical and similar pictures in a folder
    Duplicates {
        folder: PathBuf,
        /// Include subfolders
        #[arg(short, long)]
        recursive: bool,
        /// 0 finds pictures that look identical, 16 finds loosely similar ones
        #[arg(short, long, default_value_t = 6, value_parser = clap::value_parser!(u32).range(0..=16))]
        tolerance: u32,
        /// Print JSON
        #[arg(long)]
        json: bool,
    },
    /// Start the MCP server on the standard input and output
    Mcp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Keep the format of the source
    Same,
    /// Pick the smallest of JPEG, WebP and AVIF
    Auto,
    Jpeg,
    Png,
    Webp,
    Avif,
    Jxl,
    Tiff,
    Gif,
    Bmp,
    Ico,
}

impl std::fmt::Display for Format {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Same => "same",
            Self::Auto => "auto",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Avif => "avif",
            Self::Jxl => "jxl",
            Self::Tiff => "tiff",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Ico => "ico",
        })
    }
}

impl Format {
    fn output(self) -> OutputFormat {
        match self {
            Self::Same => OutputFormat::Same,
            Self::Auto => OutputFormat::Auto,
            Self::Jpeg => OutputFormat::Jpeg,
            Self::Png => OutputFormat::Png,
            Self::Webp => OutputFormat::Webp,
            Self::Avif => OutputFormat::Avif,
            Self::Jxl => OutputFormat::Jxl,
            Self::Tiff => OutputFormat::Tiff,
            Self::Gif => OutputFormat::Gif,
            Self::Bmp => OutputFormat::Bmp,
            Self::Ico => OutputFormat::Ico,
        }
    }
}

#[derive(Debug, Args)]
pub struct BatchArgs {
    /// Image files or folders
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Folder for the results; by default a "PixelIt" folder beside each source
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Include subfolders
    #[arg(short, long)]
    pub recursive: bool,
    /// Replace existing files; the replaced files go to the Recycle Bin
    #[arg(long)]
    pub overwrite: bool,
    /// Worker threads; all processor cores by default
    #[arg(short = 'j', long)]
    pub threads: Option<usize>,
    /// Print the result as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ConvertArgs {
    #[command(flatten)]
    pub batch: BatchArgs,
    /// Output format
    #[arg(short, long, value_enum, default_value_t = Format::Same)]
    pub format: Format,
    /// Quality from 1 to 100 for lossy formats
    #[arg(short, long, value_parser = clap::value_parser!(u8).range(1..=100))]
    pub quality: Option<u8>,
    /// Lossless compression where the format has it
    #[arg(long)]
    pub lossless: bool,
    /// Shrink to fit this width in pixels
    #[arg(long)]
    pub max_width: Option<u32>,
    /// Shrink to fit this height in pixels
    #[arg(long)]
    pub max_height: Option<u32>,
    /// Make every file fit into this many kilobytes
    #[arg(long)]
    pub target_kb: Option<u64>,
    /// Remove EXIF, GPS and other metadata
    #[arg(long)]
    pub strip_metadata: bool,
    /// File name template, for example "{name}-{width}"
    #[arg(long)]
    pub name: Option<String>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Recipe id, recipe name or path of an exported recipe file
    pub recipe: String,
    #[command(flatten)]
    pub batch: BatchArgs,
}

/// Whether the first argument names a command. Anything else given to
/// `PixelIt.exe` is a file to open in the window.
pub fn is_command(argument: &str) -> bool {
    matches!(argument, "help" | "-h" | "--help" | "-V" | "--version")
        || Cli::command()
            .get_subcommands()
            .any(|command| command.get_name() == argument)
}

/// Standard output that survives a closed pipe.
fn say(text: impl AsRef<str>) {
    let _ = writeln!(io::stdout(), "{}", text.as_ref());
}

fn warn(text: impl AsRef<str>) {
    let _ = writeln!(io::stderr(), "{}", text.as_ref());
}

fn megabytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => say(text),
        Err(error) => warn(format!("error: {error}")),
    }
}

/// The pipeline of `convert`.
fn convert_pipeline(arguments: &ConvertArgs) -> Pipeline {
    let mut pipeline = Pipeline::default();
    let output = &mut pipeline.output;
    output.format = arguments.format.output();
    if let Some(quality) = arguments.quality {
        output.quality = quality;
    }
    output.lossless = arguments.lossless;
    if let Some(kilobytes) = arguments.target_kb.filter(|value| *value > 0) {
        output.target = Target::Bytes {
            bytes: kilobytes.saturating_mul(1024),
        };
    }
    if arguments.strip_metadata {
        output.metadata.mode = MetadataMode::Strip;
    }
    if let Some(name) = arguments.name.as_deref().filter(|name| !name.is_empty()) {
        name.clone_into(&mut output.naming);
    }
    let width = arguments.max_width.unwrap_or(0);
    let height = arguments.max_height.unwrap_or(0);
    if width > 0 || height > 0 {
        pipeline.steps.push(Step::new(
            "resize",
            Op::Resize(Resize {
                mode: ResizeMode::Fit,
                width,
                height,
                ..Resize::default()
            }),
        ));
    }
    pipeline
}

/// Finds a recipe by the path of an exported file, by id or by name.
fn find_recipe(engine: &Engine, key: &str) -> Result<recipes::Recipe> {
    let path = Path::new(key);
    if path.is_file() {
        return recipes::import(path);
    }
    Recipes::load(&engine.paths)
        .find(key)
        .ok_or_else(|| Error::new("recipeNotFound").with("name", key))
}

fn run_batch(
    engine: &Engine,
    title: &str,
    mut pipeline: Pipeline,
    arguments: &BatchArgs,
) -> Result<BatchReport> {
    if let Some(folder) = &arguments.output {
        let folder = std::path::absolute(folder)?;
        pipeline.output.destination = Destination::Folder {
            path: folder.to_string_lossy().into_owned(),
        };
    }
    if arguments.overwrite {
        pipeline.output.conflict = Conflict::Overwrite;
    }
    let skip = match &pipeline.output.destination {
        Destination::Subfolder { name } => Some(name.as_str()),
        Destination::SameFolder | Destination::Folder { .. } => None,
    };
    let inputs: Vec<PathBuf> = arguments
        .inputs
        .iter()
        .map(|input| std::path::absolute(input).unwrap_or_else(|_| input.clone()))
        .collect();
    for input in &inputs {
        if !input.exists() {
            return Err(Error::new("notFound").with("path", input.display()));
        }
    }
    let items = scan::collect(&inputs, arguments.recursive, skip);
    if items.is_empty() {
        return Err(Error::new("noImages"));
    }
    let settings = engine.settings();
    let request = BatchRequest {
        title: title.to_owned(),
        kind: String::from("cli"),
        items,
        pipeline,
        threads: arguments.threads.unwrap_or(settings.worker_threads()),
        memory_limit: u64::from(settings.memory_limit_mb) * 1024 * 1024,
    };
    batch::run(
        &engine.runner(),
        Some(&engine.paths),
        &request,
        &JobContext::detached(),
    )
}

fn report_batch(report: &BatchReport, json: bool) -> i32 {
    if json {
        print_json(report);
    } else {
        for item in &report.items {
            match (&item.outcome, &item.error) {
                (Some(outcome), _) => say(format!(
                    "{} -> {} ({}x{}, {})",
                    item.source.display(),
                    outcome.output.display(),
                    outcome.width,
                    outcome.height,
                    megabytes(outcome.output_bytes)
                )),
                (None, Some(error)) => warn(format!("{}: {error}", item.source.display())),
                (None, None) => {}
            }
        }
        let saved = if report.source_bytes > 0 && report.output_bytes < report.source_bytes {
            format!(
                ", {}% smaller",
                100 - report.output_bytes * 100 / report.source_bytes
            )
        } else {
            String::new()
        };
        say(format!(
            "Done: {}, skipped: {}, failed: {}. {} -> {}{saved}",
            report.done,
            report.skipped,
            report.failed,
            megabytes(report.source_bytes),
            megabytes(report.output_bytes)
        ));
    }
    if report.failed > 0 {
        EXIT_FAILED
    } else {
        EXIT_OK
    }
}

fn list_recipes(engine: &Engine, json: bool) -> i32 {
    let recipes = Recipes::load(&engine.paths).all();
    if json {
        print_json(&recipes);
    } else {
        for recipe in &recipes {
            let steps: Vec<&str> = recipe
                .pipeline
                .steps
                .iter()
                .filter(|step| step.enabled)
                .map(|step| step.op.kind())
                .collect();
            say(format!(
                "{}\t{}\t{}",
                recipe.id,
                recipe.name,
                if steps.is_empty() {
                    String::from("-")
                } else {
                    steps.join(", ")
                }
            ));
        }
    }
    EXIT_OK
}

fn info(files: &[PathBuf], json: bool) -> i32 {
    let mut code = EXIT_OK;
    let mut rows = Vec::new();
    for file in files {
        match probe_file(file) {
            Ok(info) => {
                let bytes = fs::metadata(file).map_or(0, |meta| meta.len());
                if json {
                    let mut row = serde_json::to_value(&info).unwrap_or(Value::Null);
                    row["path"] = json!(file);
                    row["bytes"] = json!(bytes);
                    rows.push(row);
                } else {
                    let mut notes = Vec::new();
                    if info.has_alpha {
                        notes.push(String::from("alpha"));
                    }
                    if info.frames > 1 {
                        notes.push(format!("{} frames", info.frames));
                    }
                    for (present, name) in [
                        (info.has_exif, "EXIF"),
                        (info.has_icc, "ICC"),
                        (info.has_xmp, "XMP"),
                    ] {
                        if present {
                            notes.push(name.to_owned());
                        }
                    }
                    say(format!(
                        "{}: {} {}x{} {}-bit, {}{}",
                        file.display(),
                        info.format.map_or("?", ImageFormat::name),
                        info.width,
                        info.height,
                        info.bits,
                        megabytes(bytes),
                        if notes.is_empty() {
                            String::new()
                        } else {
                            format!(", {}", notes.join(", "))
                        }
                    ));
                }
            }
            Err(error) => {
                code = EXIT_FAILED;
                warn(format!("{}: {error}", file.display()));
            }
        }
    }
    if json {
        print_json(&rows);
    }
    code
}

fn compare(
    engine: &Engine,
    file: &Path,
    formats: &[Format],
    quality: u8,
    json: bool,
) -> Result<()> {
    let formats: Vec<ImageFormat> = formats
        .iter()
        .filter_map(|format| format.output().fixed())
        .collect();
    let cancel = CancelToken::new();
    let decoded = engine.runner().load(file, 0, &cancel)?;
    let mut spec = engine.settings().output;
    spec.quality = quality;
    let rows =
        compress::compare_formats(&decoded.media, &spec, &formats, &decoded.metadata, &cancel);
    if json {
        print_json(&rows);
        return Ok(());
    }
    let source = fs::metadata(file)?.len();
    say(format!("{} ({})", file.display(), megabytes(source)));
    for row in &rows {
        match &row.error {
            Some(error) => say(format!("  {:<5} {error}", row.format.name())),
            None => say(format!(
                "  {:<5} {:>10}  {:>4}%  score {}",
                row.format.name(),
                megabytes(row.bytes),
                row.bytes
                    .saturating_mul(100)
                    .checked_div(source)
                    .unwrap_or(0),
                row.score
                    .map_or_else(|| String::from("-"), |score| format!("{score:.1}"))
            )),
        }
    }
    Ok(())
}

fn find_duplicates(
    engine: &Engine,
    folder: &Path,
    recursive: bool,
    tolerance: u32,
    json: bool,
) -> Result<()> {
    if !folder.is_dir() {
        return Err(Error::new("notFound").with("path", folder.display()));
    }
    let files = scan::folder_files(folder, recursive, None);
    let groups = duplicates::find(&engine.runner(), &files, tolerance, &JobContext::detached())?;
    if json {
        print_json(&groups);
        return Ok(());
    }
    for (index, group) in groups.iter().enumerate() {
        say(format!(
            "Group {} ({})",
            index + 1,
            if group.exact {
                "identical files"
            } else {
                "similar pictures"
            }
        ));
        for file in &group.files {
            say(format!(
                "  {} ({}x{}, {})",
                file.path.display(),
                file.width,
                file.height,
                megabytes(file.bytes)
            ));
        }
    }
    say(format!("Files: {}, groups: {}", files.len(), groups.len()));
    Ok(())
}

/// Runs a parsed command and returns the exit code of the process.
pub fn execute(cli: Cli, engine: &Arc<Engine>, ai: &Arc<Ai>) -> i32 {
    let outcome: Result<i32> = match cli.command {
        Command::Convert(arguments) => run_batch(
            engine,
            "convert",
            convert_pipeline(&arguments),
            &arguments.batch,
        )
        .map(|report| report_batch(&report, arguments.batch.json)),
        Command::Run(arguments) => find_recipe(engine, &arguments.recipe)
            .and_then(|recipe| {
                // Built-in recipes get their names in the window; here the id serves.
                let title = if recipe.name.is_empty() {
                    recipe.id
                } else {
                    recipe.name
                };
                run_batch(engine, &title, recipe.pipeline, &arguments.batch)
            })
            .map(|report| report_batch(&report, arguments.batch.json)),
        Command::Recipes { json } => Ok(list_recipes(engine, json)),
        Command::Info { files, json } => Ok(info(&files, json)),
        Command::Compare {
            file,
            formats,
            quality,
            json,
        } => compare(engine, &file, &formats, quality, json).map(|()| EXIT_OK),
        Command::Duplicates {
            folder,
            recursive,
            tolerance,
            json,
        } => find_duplicates(engine, &folder, recursive, tolerance, json).map(|()| EXIT_OK),
        Command::Mcp => {
            let context = ToolContext::new(Arc::clone(engine), Arc::clone(ai));
            pixelit_mcp::server::serve(&context, io::stdin().lock(), io::stdout());
            ai.release();
            Ok(EXIT_OK)
        }
    };
    match outcome {
        Ok(code) => code,
        Err(error) => {
            warn(format!("error: {error}"));
            EXIT_FAILED
        }
    }
}

/// Parses the command line (the program name first) and runs it.
pub fn run<I, T>(arguments: I, engine: &Arc<Engine>, ai: &Arc<Ai>) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    match Cli::try_parse_from(arguments) {
        Ok(cli) => execute(cli, engine, ai),
        Err(error) => {
            // Help and version requests arrive here too and are not failures.
            let code = if error.use_stderr() {
                EXIT_USAGE
            } else {
                EXIT_OK
            };
            let _ = error.print();
            code
        }
    }
}

#[cfg(test)]
mod tests {
    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::{Color, Depth, Image};
    use pixelit_engine::Paths;

    use super::*;

    fn setup() -> (tempfile::TempDir, Arc<Engine>, Arc<Ai>, PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::open(Paths::at(folder.path().join("data"))).unwrap());
        let ai = Arc::new(Ai::open(engine.paths.clone()));
        let pictures = folder.path().join("pictures");
        fs::create_dir_all(&pictures).unwrap();
        (folder, engine, ai, pictures)
    }

    fn picture(path: &Path, width: u32, height: u32) {
        let image = Image::new(width, height, Depth::Eight, Color::rgb(200, 90, 30)).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn the_command_line_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn commands_are_told_from_files() {
        for command in [
            "convert",
            "run",
            "recipes",
            "info",
            "compare",
            "duplicates",
            "mcp",
        ] {
            assert!(is_command(command), "{command}");
        }
        assert!(is_command("--help"));
        assert!(is_command("--version"));
        assert!(!is_command("C:\\Photos\\cat.jpg"));
        assert!(!is_command("convert.png"));
    }

    #[test]
    fn convert_options_build_the_pipeline() {
        let cli = Cli::try_parse_from([
            "PixelIt",
            "convert",
            "a.png",
            "b",
            "-f",
            "webp",
            "-q",
            "70",
            "--max-width",
            "800",
            "--target-kb",
            "150",
            "--strip-metadata",
            "-o",
            "out",
            "-r",
            "--name",
            "{name}-web",
        ])
        .unwrap();
        let Command::Convert(arguments) = cli.command else {
            panic!("not convert");
        };
        assert_eq!(arguments.batch.inputs.len(), 2);
        assert!(arguments.batch.recursive);
        let pipeline = convert_pipeline(&arguments);
        assert_eq!(pipeline.output.format, OutputFormat::Webp);
        assert_eq!(pipeline.output.quality, 70);
        assert_eq!(pipeline.output.target, Target::Bytes { bytes: 153_600 });
        assert_eq!(pipeline.output.metadata.mode, MetadataMode::Strip);
        assert_eq!(pipeline.output.naming, "{name}-web");
        assert_eq!(pipeline.steps.len(), 1);
        assert!(Cli::try_parse_from(["PixelIt", "convert", "a.png", "-q", "0"]).is_err());
        assert!(Cli::try_parse_from(["PixelIt", "convert"]).is_err());
    }

    #[test]
    fn files_are_converted_into_a_folder() {
        let (folder, engine, ai, pictures) = setup();
        picture(&pictures.join("a.png"), 300, 200);
        picture(&pictures.join("b.png"), 100, 100);
        let out = folder.path().join("out");
        let code = run(
            [
                OsString::from("PixelIt"),
                "convert".into(),
                pictures.clone().into(),
                "-f".into(),
                "jpeg".into(),
                "--max-width".into(),
                "150".into(),
                "-o".into(),
                out.clone().into(),
                "--json".into(),
            ],
            &engine,
            &ai,
        );
        assert_eq!(code, EXIT_OK);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 2);
        let info = probe_file(&out.join("a.jpg")).unwrap();
        assert_eq!((info.width, info.height), (150, 100));
    }

    #[test]
    fn recipes_run_by_id_and_problems_set_the_exit_code() {
        let (folder, engine, ai, pictures) = setup();
        picture(&pictures.join("a.png"), 64, 64);
        let id = Recipes::load(&engine.paths).all()[0].id.clone();
        let source = pictures.join("a.png");
        let run_with = |recipe: &str, input: &Path| {
            run(
                [
                    OsString::from("PixelIt"),
                    "run".into(),
                    recipe.into(),
                    input.into(),
                ],
                &engine,
                &ai,
            )
        };
        assert_eq!(run_with(&id, &source), EXIT_OK);
        assert_eq!(run_with("no such recipe", &source), EXIT_FAILED);
        assert_eq!(
            run_with(&id, &folder.path().join("missing.png")),
            EXIT_FAILED
        );
        assert_eq!(run(["PixelIt", "frobnicate"], &engine, &ai), EXIT_USAGE);
        assert_eq!(run(["PixelIt", "--version"], &engine, &ai), EXIT_OK);
        assert_eq!(run(["PixelIt", "recipes"], &engine, &ai), EXIT_OK);
    }

    #[test]
    fn info_compare_and_duplicates_work() {
        let (_folder, engine, ai, pictures) = setup();
        picture(&pictures.join("a.png"), 64, 64);
        picture(&pictures.join("b.png"), 64, 64);
        let a: OsString = pictures.join("a.png").into();
        let folder: OsString = pictures.clone().into();
        assert_eq!(
            run(
                [OsString::from("PixelIt"), "info".into(), a.clone()],
                &engine,
                &ai
            ),
            EXIT_OK
        );
        assert_eq!(
            run(
                [
                    OsString::from("PixelIt"),
                    "info".into(),
                    "missing.png".into()
                ],
                &engine,
                &ai
            ),
            EXIT_FAILED
        );
        assert_eq!(
            run(
                [
                    OsString::from("PixelIt"),
                    "compare".into(),
                    a,
                    "-f".into(),
                    "jpeg,png".into(),
                ],
                &engine,
                &ai
            ),
            EXIT_OK
        );
        assert_eq!(
            run(
                [OsString::from("PixelIt"), "duplicates".into(), folder],
                &engine,
                &ai
            ),
            EXIT_OK
        );
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(megabytes(512), "0.5 KB");
        assert_eq!(megabytes(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }
}
