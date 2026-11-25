# PixelIt

[Русский](README.ru.md)

An image toolkit for Windows: a viewer, a non-destructive editor, batch processing with recipes, set generators and local AI models. Everything runs on your computer. The same operations are available in the window, from the command line and to AI agents through MCP.

The interface is available in German, English, Spanish, French, Japanese, Korean, Polish, Portuguese, Russian, Turkish and Simplified Chinese. The application has no account. It uses the network only for what you asked for: update checks, model downloads, and requests to the servers and services you set up yourself.

## Download

The latest version is on the [releases](https://github.com/Crazymoralfreak/PixelIt/releases/latest) page:

- `PixelIt_<version>_x64-setup.exe` — the installer. It installs the application for the current user, no administrator rights needed.
- `PixelIt_<version>_x64-portable.zip` — the portable version. Unpack the `PixelIt` folder anywhere and start `PixelIt.exe`. The `portable` file beside the program makes it keep all its data in the `data` folder next to itself.

The files are not signed with a code signing certificate, so Windows SmartScreen shows a warning on the first start.

## What it can do

- **Viewer.** Folders with thumbnails, sorting and a filter; size, format, bit depth and metadata of every image. Files open by drag and drop, from a dialog, from the clipboard (Ctrl+V), from Explorer and from launch arguments.
- **From the web.** Images by link — a Pinterest pin, any web page or the direct address of an image — and search in the Openverse and Wikimedia Commons catalogues of freely licensed images. No account and no keys are needed. A download opens in the editor or goes to the batch; its source, author and license are kept in the history. A link can be pasted with Ctrl+V from anywhere in the application.
- **Formats.** Reads JPEG, PNG, WebP, AVIF, JPEG XL, HEIC, TIFF, GIF, BMP, ICO, camera RAW, SVG, PDF and PSD. Writes JPEG, PNG, WebP, AVIF, JPEG XL, TIFF, GIF, BMP and ICO. 16 bits per channel are kept where the format has them.
- **Editor.** A stack of steps that never changes the source: resize, crop, rotate, flip, adjustments, filters, watermarks, annotations. Before and after comparison, undo and redo, a size estimate and a comparison of formats before saving.
- **Batch.** A queue of files and folders processed in parallel, with pause, cancel and the remaining time. A target file size, quality chosen by visual similarity, automatic choice of the smallest format.
- **Recipes and watch folders.** A recipe is a saved set of steps and output settings; it can be exported as a file. A watch folder processes every new image with a recipe.
- **History.** Every run is recorded and can be undone: new files go to the Recycle Bin, replaced files come back.
- **Tools.** Icon sets, social media sizes, responsive web sets with ready HTML, collage, PDF from images, animated GIF, WebP and APNG, search for identical and similar images, screen capture by a hotkey.
- **Windows.** Tray icon, autostart, file associations, an "Open in PixelIt" item in the Explorer context menu, automatic updates.

## Models and AI

Nothing of this is installed with the application. Models and components are downloaded on the "Models and AI" page when you ask for them, each one checked against its checksum.

- **Local models** (ONNX Runtime with DirectML, on the video card or the processor): background removal, upscaling, object removal, face restoration. They run in a separate process, `pixelit-worker.exe`, so a failure inside a model or a video driver does not take the application down.
- **Image generation** through a ComfyUI or Forge / AUTOMATIC1111 server, the built-in stable-diffusion.cpp with your model file, or an external service with an OpenAI-compatible API and your key.
- **Understanding images** through a model with an OpenAI-compatible API (Ollama, LM Studio or an external service): descriptions, alt text, keywords, file names, text recognition. The answer can be written into the metadata of the file.
- **Assistant.** A chat that calls the tools of PixelIt and of the external MCP servers you connect.

Privacy rules:

- External services are off until you switch them on, and PixelIt asks before it sends an image or a prompt to one. A server on this computer (loopback address) counts as local.
- Keys are kept in the Windows Credential Manager, not in the settings files.
- The assistant and external agents work only inside the folders you allowed. Overwriting, deleting, downloading from the web (models, components, images) and using external services are separate permissions, all off by default.
- The "From the web" page uses the network only when you act: the link you pasted, the search catalogue you chose and the addresses of the images found. The text of a query goes to the chosen catalogue only. Addresses of this computer and of the local network are closed to agents.

## Updates

On start, the installed version asks GitHub whether a new version is out. PixelIt tells about the version it found and asks whether to install it: "Update", "Later" or "Skip this version". Nothing is downloaded without consent. After you agree, the installer is downloaded, its signature is checked, and it runs when no job is running; after the installation PixelIt starts again. Recipes and settings are kept.

- The check is turned off in the settings. Without it PixelIt does not contact GitHub on its own.
- The "beta" channel in the settings offers pre-release versions too.
- The portable version does not update itself: it only reports that a new version is out.
- After an update the "What's new" window shows the changes of the new version from `CHANGELOG.md`; the whole history is on the "About" page.

## Command line

`PixelIt.exe` is also the command line tool: when its first argument is a command, it runs the command without opening the window.

```
PixelIt convert photos --format webp --quality 80 --max-width 1920 --output out
PixelIt run web photos --recursive
PixelIt recipes
PixelIt info photo.jpg --json
PixelIt compare photo.png --formats jpeg,webp,avif,jxl
PixelIt duplicates photos --recursive --tolerance 6
PixelIt help
```

- `convert` — convert, compress or shrink; `run` — process with a saved recipe (its id, its name or an exported file).
- New files are written into a `PixelIt` folder beside each source unless `--output` names another folder. Existing files are replaced only with `--overwrite`; the replaced files go to the Recycle Bin.
- `--json` prints the result for scripts. The exit code is 0 on success, 1 when at least one file failed, 2 for a wrong command line.
- `PixelIt.exe` is a window program, so an interactive `cmd` or PowerShell does not wait for it and shows the prompt before the output ends. In a script, wait for it explicitly: `start /wait PixelIt.exe ...` in `cmd`, or pipe the output (`PixelIt.exe info a.jpg | Out-String`) in PowerShell.

## MCP server

`PixelIt.exe mcp` is an MCP server on the standard input and output. An agent (Cursor, Claude Desktop and other MCP clients) gets the same tools as the built-in assistant: `list_images`, `image_info`, `list_recipes`, `run_recipe`, `convert`, `run_pipeline`, `compare_formats`, `find_duplicates`, `undo`, `ai_status`, `remove_background`, `upscale`, `restore_faces`, `generate_image`, `describe_image`, `install_component`, `find_web_images`, `download_web_image`.

The "Models and AI" page, tab "MCP", shows the ready text for the settings of the client. Its form:

```json
"mcpServers": {
  "pixelit": {
    "command": "path to PixelIt.exe",
    "args": ["mcp"]
  }
}
```

The same tab sets what agents may do: the allowed folders and the separate permissions to overwrite, delete, download from the web and use external services. By default the server writes new files only and downloads nothing.

PixelIt is an MCP client as well: the same tab connects external MCP servers (a program over the standard streams or an address with the Streamable HTTP transport), with a permission for every tool: allow, ask or deny.

## Requirements

- Windows 10 version 1809 or newer, or Windows 11, x64.
- The WebView2 runtime. Windows 11 always has it. On a machine without WebView2 the installer downloads it; the portable version does not start without it.
- HEIC files are read by the Windows codecs: they need the "HEIF Image Extensions" and "HEVC Video Extensions" from the Microsoft Store.
- PDF files are read by the PDFium component, downloaded on the "Models and AI" page.
- The video card is used through DirectML when it has one; without it the models run on the processor, slower.

## Data

Recipes, settings, the journal, chats, downloaded models and the thumbnail cache are in `%AppData%\PixelIt`. The portable version keeps them in the `data` folder beside `PixelIt.exe`. Images from the web are saved there too, in the `downloads` folder, until the settings name another one. Only one instance runs at a time; a second start hands its files to the first.

When the program is uninstalled the folder stays. A separate box in the uninstall window deletes it; it is off by default.

## Building

You need:

- Node.js 20.19 or newer (or 22.12 and newer);
- Rust 1.98 or newer with the `x86_64-pc-windows-msvc` toolchain;
- Microsoft C++ Build Tools and NASM (the JPEG, WebP and AV1 encoders have C and assembly parts).

```
npm install
npm run tauri build
```

After the build:

- the installer: `target\release\bundle\nsis\PixelIt_<version>_x64-setup.exe`;
- the program: `target\release\PixelIt.exe`. The local models also need `pixelit-worker.exe` beside it: `cargo build --release -p pixelit-worker`.

A portable copy is a folder with `PixelIt.exe`, `pixelit-worker.exe` and an empty file named `portable`. Git ignores the `portable` folder at the root of the repository, so a copy for a try-out can be kept there.

Development mode:

```
npm run tauri dev
```

## Releasing a version

1. Put the new version number into three files: `package.json`, `src-tauri/tauri.conf.json` and the root `Cargo.toml` (`[workspace.package]`), and add its section to `CHANGELOG.md` and `CHANGELOG.ru.md`.
2. Commit the changes and tag the commit `v<version>`, for example `v1.1.0`.
3. Push the tag to GitHub.

The tag starts `.github/workflows/release.yml`: it checks that the number in the tag matches the three files, runs the tests, builds and signs the installer, builds the portable archive and attaches three files to the release of the tag: the installer, the portable archive and `latest.json`. The description of the release is the section of the version in `CHANGELOG.md`.

`latest.json` describes the update: the version, the link to the installer and its signature. Installed copies read it from `releases/latest/download/latest.json`, so they see only a release that GitHub considers the latest: not a draft and not a pre-release.

A tag with a suffix, such as `v1.1.0-beta.1`, makes a pre-release. Copies on the "beta" channel read the manifest of the permanent release `beta-channel`, which every release refreshes.

### The update signing key

The application installs only an update whose signature matches the public key in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`). The private key and its password are not in the repository. They belong in the GitHub Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

If the private key is lost, installed copies cannot be updated: a version with a new public key has to be released and installed by hand.

Signing is turned on by `src-tauri/tauri.release.conf.json`, which only the release build uses. An ordinary local build needs no key; its installer is not signed.

## Checks

```
npm run typecheck
npm run i18n:check
npm run licenses:check
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

`.github/workflows/ci.yml` runs them on every push and pull request, together with `cargo deny` (licences and sources of dependencies, `deny.toml`) and the unit tests of the core under Miri.

- **Properties.** `crates/pixelit-core/tests/properties.rs` checks resizing and geometry on generated values.
- **Damaged files.** `crates/pixelit-codecs/tests/damaged.rs` cuts and overwrites files of every format; reading must end with an image or an error, never with a crash. The AV1 decoder can stop the whole process on a damaged file, so PixelIt decodes AVIF in a short-lived copy of itself (`PixelIt.exe --decode-avif`) and reports such a file as unreadable.
- **Golden samples.** `crates/pixelit-codecs/tests/golden.rs` compares the result of every operation with a picture stored in `crates/pixelit-codecs/tests/golden`. After an intended change of an operation the pictures are written again: by the "Golden samples" workflow, started by hand, or by running the test with `PIXELIT_BLESS=1`.
- **Fuzzing.** `fuzz/` holds `cargo fuzz` targets for the decoders, the metadata code and the recipe reader; `fuzz.yml` runs them weekly.
- **Benchmarks.** `bench.yml` runs the criterion benchmarks weekly and by hand.

`src/licenses.json`, the list on the "About" page, is generated from `Cargo.lock` and `package.json` by `npm run licenses`. The icons in `src-tauri/icons` are drawn by `npm run icons`.

## Layout

- `crates/pixelit-core` — pixel buffers, the operations and the recipe model (`Pipeline`).
- `crates/pixelit-codecs` — reading and writing of the formats, colour management, metadata.
- `crates/pixelit-engine` — running recipes, batch, the job queue, storage, watch folders, generators, the journal.
- `crates/pixelit-ai` — components and models, the worker client, generation providers, vision models.
- `crates/pixelit-worker` — the separate process that hosts ONNX Runtime.
- `crates/pixelit-tools` — one registry of tools for the assistant and the MCP server.
- `crates/pixelit-mcp` — the MCP server and the client for external MCP servers.
- `crates/pixelit-cli` — the command line.
- `src-tauri` — the application: window, tray, hotkeys, updates, Windows integration, commands for the interface.
- `src` — the interface in React and TypeScript: pages in `pages`, dictionaries in `i18n`, types mirroring the Rust model in `types`.

## Licence

MIT, see [LICENSE](LICENSE). The licences of the libraries PixelIt is built from are listed on the "About" page; the RAW decoder `rawler` is LGPL-2.1. Downloaded models and components have their own licences, shown next to each of them.
