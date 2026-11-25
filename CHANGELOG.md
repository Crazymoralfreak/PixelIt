# Changelog

## [Unreleased]

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
