# Changelog

## [Unreleased]

### Applications

- ComfyUI and Forge can be installed, started and stopped from Models and AI. A folder you already have, including one from Stability Matrix, is used as it is.
- Checkpoints, LoRA and the other model folders are shared. ComfyUI receives all of them. The built-in generator lists checkpoints from those folders.
- ComfyUI and Forge installed from this page can check for a newer release of that program. PixelIt's own updater is unchanged.
- Models can be searched on CivitAI, CivArchive, Hugging Face and OpenModelDB and downloaded into the matching folder.
- A ComfyUI Desktop data folder brings in the program itself, its model folders and the port it last used.
- The studio can enlarge ×2 or ×4 as part of a generation. When the enlargement model is missing, the Enlarge buttons offer to install it instead of staying grey.
- A model download in a search card shows the bar, the share and the time left; Download becomes Cancel for that time.
- The start log folds, scrolls and follows new lines. It can be copied, cleared and shown as a file. Each start begins the log again; the one before it stays beside it as `.prev.log`.
- The search for local servers also finds ComfyUI Desktop and Fooocus, on whichever port they took. Fooocus is listed as found; it is not a generator yet.
- Enlargement can use the built-in model, an ONNX file from the upscaler folders, or a model a running ComfyUI or Forge reports. By default the generator then paints detail back in; Fast skips that pass.
- An upscaler on OpenModelDB that has an ONNX file is marked as working without ComfyUI, and that file can be the one downloaded.
- Painting detail through ComfyUI sends the frame fields its tiled encoder now requires.
- A model card shows the folder a download will use, and the full path once the file is there. A download that fails stays explained on the card.
- ComfyUI model lists are also read in the form its newer nodes answer in, so an upscaler of a running ComfyUI can be chosen.
- Show file opens the folder with the file even when the saved path held a forward slash; model folders are written with the system's separators.

### Studio

- ComfyUI and Forge take a sampler, a scheduler and LoRA files with a strength each; ComfyUI also takes a separate VAE. The running generator gives the lists; the choice is remembered for the model and written into the history. With nothing chosen, a generation runs as before.
- A result and the picture dialog show the picture at the resolution of the screen. Before, it was a thumbnail of up to 640 px stretched over the column, so the picture looked softer than its file.
- Add detail, on a result and in the picture dialog: the generator goes over the picture once more and paints in fine detail, at the same size. The strength is the one in the upscale settings.
- The picture dialog takes the whole window: the wheel zooms the picture and a drag moves it. The before and after curtain is labelled, its halves lie exactly over each other, and it is shown right on the result after adding detail, upscaling or an edit.
- A corner of the form shows whether ComfyUI or Forge on this computer answers; an installed program can be started and stopped right there. Once it has started, the model lists refresh on their own.
- The history is shown grouped by request, as a grid or as a list; the view is remembered. The prompt can be copied with a button in every view.
- ComfyUI understands a prompt written for Forge: an embedding named by its file is passed as `embedding:name`, and `BREAK` splits the prompt into pieces that are encoded separately. Before, ComfyUI read both as plain words.
- Embeddings are chosen where the sampler, the VAE and LoRA files are: each has a weight and a side, the prompt or the negative prompt. The chosen ones are written into the request by themselves, in the form the generator reads; an embedding the text already names is not added twice.
- The negative prompt after the styles is shown with the final prompt and in the picture dialog.
- The final prompt and the picture dialog show the chosen embeddings and LoRA files: an embedding in the form the generator reads, a LoRA file as `<lora:name:weight>`. This is the display only; a generation is sent what it was sent before.
- A style of your own is saved under any name. A name without Latin letters, such as one in Russian or Japanese, or a name that matches a built-in style was refused with "A built-in style cannot be changed or removed".

### Editor

- A new AI detail step: the generator goes over the picture or over a painted area and paints in fine detail, and what the picture shows stays. On saving, the file is painted at its full size instead of being stretched from the reduced preview. For a generated picture the prompt is taken from the file itself.
- The prompt of the AI detail step can be left empty. An empty field sends the generator `highly detailed, sharp focus, intricate details`; a text of your own describes the picture and takes the place of that line.
- The AI detail step takes a negative prompt, steps and prompt adherence, and for ComfyUI and Forge a sampler, a scheduler, a VAE, LoRA files and embeddings. As in the studio fills in what the studio remembers for the model. Empty fields come from the settings. The AI edit step is unchanged.
- A large picture is encoded and decoded in tiles by ComfyUI during an edit or a detail pass too, so it fits the video memory.

### macOS

- A version tag also builds disk images `PixelIt_<version>_MacOS_Silicon.dmg` and `PixelIt_<version>_MacOS_Intel.dmg`, with the model worker inside the app. The image is signed locally (ad-hoc), so Gatekeeper asks before the first open; notarization can be switched on later with Apple secrets.
- Local ONNX models use Core ML on Apple Silicon. PDFium, HEIC (via `sips`), screen capture (with Screen Recording permission), undo of replaced files, hardware detection, Metal stable-diffusion.cpp, Finder open/reopen, the menu bar Quit path, Command shortcuts and Keychain texts work on Mac. The Explorer context menu stays on Windows. Intel Macs hide components that have no build for that CPU.
- Installed copies under Applications can update themselves; `latest.json` lists Windows and both Mac architectures.
- The Windows installer and uninstaller carry the application icon.

## [0.0.3] - 2026-11-25

### Studio

- The generator and its model are chosen in the studio and remembered. The same choice is the default for the rest of the program; an "AI edit" step can override it.
- One request makes 1 to 4 pictures. The shape is chosen with buttons, and the size, steps and CFG are remembered for each model.
- "Improve prompt" asks the assistant model to rewrite the text. "Restore" brings the previous text back.
- Styles add text to the prompt and to what to avoid. Built-in styles can be duplicated; your own can be created, changed, imported and exported as Fooocus JSON.
- "Edit" changes a whole picture or the area painted with the brush, and can extend the canvas so the generator paints past the edges. The original picture is put back outside that area.
- While a picture is generated, the tile shows the step and, when the generator sends one, a preview.
- Each result can make more variants, a similar picture, an edit, or a ×2 / ×4 enlargement with the local upscale model.
- The history lists every generated picture, with search, favourites, a large view and a before-and-after wipe. Several can be saved to a folder or moved to the recycle bin.
- A PNG can carry the prompt in the text block other tools read. Dropping such a file on the studio fills the form. The switch is on by default.

### Editor

- Exporting an "AI edit" that is already shown no longer asks to send the picture outside: the result is taken from the cache, and the agreement given for the preview is reused.

### Dependencies

- `tungstenite` (MIT or Apache-2.0) is used for the ComfyUI progress connection. If it cannot connect, generation continues without the live preview.

## [0.0.2] - 2026-11-25

### Images from the web

- A new page, "From the web". By link: a Pinterest pin (a short `pin.it` link too), any web page or an image itself. An image behind a direct link is downloaded at once; for a page PixelIt shows the images found on it, the main one first and already selected. No account is needed.
- Search in the Openverse and Wikimedia Commons catalogues: freely licensed images, each shown with its author and license. No key is needed; the text of the query goes to the chosen catalogue only.
- The selection can be downloaded, downloaded and opened in the editor, or downloaded and added to the batch. A download runs as a job with progress and cancel.
- A link pasted with Ctrl+V outside input fields opens this page with the address filled in.
- The settings have a "Download folder"; until it is chosen, files are saved into the `downloads` folder in the PixelIt data.
- The history keeps the source address of every downloaded file, and its author and license when they are known; a download can be undone.
- The assistant and MCP agents have two new tools, `find_web_images` and `download_web_image`. They work only with the permission to download from the web and inside the allowed folders; addresses of this computer and of the local network are closed to agents.

### Editor

- A new step, "AI edit": the connected generator changes the whole picture or the area painted with the brush by your description. It has a strength setting and an "Another variant" button; the export uses the same result the editor shows.
- Background removal: the brush marks the areas to keep.
- Steps with a brush have "Run" and "Change the area" buttons: the step starts once the area is marked.
- The "Brush size" slider is always in sight, and the hint under it names the button that turns the brush on. The outline of the brush shows under the pointer; the [ and ] keys change the size too.
- An indicator over the picture shows that it is being processed; the error of a step stays on screen with a "Try again" button.
- Object removal: a model that does not start on the graphics card moves to the processor by itself.

### Models and AI

- The key of a server is saved as soon as it is pasted and before the list of models is asked for.
- When outside services are switched off, PixelIt offers to allow them right from the request; without a key it tells where to enter one.
- Studio: for an outside service the sizes are chosen by shape (1:1, 3:2, 2:3, 16:9, 9:16). When the service refuses a size, the nearest allowed one is asked for.

### Updates

- PixelIt asks whether to install a new version: "Update", "Later" or "Skip this version". Nothing is downloaded without consent.

### Dependencies

- The `zip` library is updated to a version without known vulnerabilities; the unmaintained `chrono` is no longer a direct dependency.

## [0.0.1] - 2026-11-24

### Viewer and import

- Folder view with thumbnails, sorting and a filter; size, format, bit depth and metadata of every image.
- Opening by drag and drop, from a dialog, from the clipboard, from Explorer and from launch arguments.
- Formats: JPEG, PNG, WebP, AVIF, JPEG XL, HEIC, TIFF, GIF, BMP, ICO, RAW, SVG, PDF, PSD.

### Editor

- A non-destructive stack of steps: resize, crop, rotate, flip, adjustments, filters, watermarks, annotations.
- Before and after comparison, undo and redo, file size estimate and format comparison before saving.
- Background removal, upscaling, object removal and face restoration with local models.

### Batch processing

- A queue of files and folders with parallel processing, pause, cancel and the remaining time.
- Recipes: built-in and your own, import and export as a single file.
- Target file size, quality chosen by visual similarity, automatic choice of the best format.
- Watch folders: new images are processed with the chosen recipe.
- A journal with undo: created files go to the Recycle Bin, replaced files come back.

### Tools

- Icon sets, social media covers, responsive web sets with ready HTML.
- Collage, PDF from images, animated GIF, WebP and APNG.
- Search for identical and similar images.
- Screen capture by a hotkey.

### Models and AI

- Component manager: models are downloaded on request and verified by checksum.
- Image generation through ComfyUI, Forge, the built-in stable-diffusion.cpp or an external service with your key.
- Descriptions, alt text, tags, file names and text recognition through a model that understands images.
- An assistant with PixelIt tools and external MCP servers.
- External services are off by default; PixelIt asks before it sends an image.

### Command line and MCP

- `PixelIt.exe` runs commands from the console: recipes, batch, conversion.
- `PixelIt.exe mcp` works as an MCP server for Cursor, Claude Desktop and other clients.

### Windows

- Tray icon, autostart, file associations, an item in the Explorer context menu.
- Automatic update of the installed version with signature verification; stable and beta channels.
- A portable version and an interface in 11 languages.
