# Changelog

## [0.0.2] - 2026-11-25

### Editor

- A new step, "AI edit": the connected generator changes the whole picture or the area painted with the brush by your description. It has a strength setting and an "Another variant" button; the export uses the same result the editor shows.
- Background removal: the brush marks the areas to keep.
- Steps with a brush have "Run" and "Change the area" buttons: the step starts once the area is marked.
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
