//! Arbitrary text given to the recipe reader. Recipes arrive from imported
//! files, from the command line and from MCP clients, so the reader must
//! refuse bad input with an error; an accepted recipe must be writable.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pixelit_core::ops::{geometry, resize};
use pixelit_core::pipeline::Op;
use pixelit_core::Pipeline;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(pipeline) = Pipeline::from_json(text) else {
        return;
    };
    serde_json::to_string(&pipeline).expect("an accepted recipe is written back");
    let _ = pipeline.uses_ai();

    // The size arithmetic of every step, on a source of an ordinary size.
    for step in &pipeline.steps {
        match &step.op {
            Op::Resize(spec) => {
                let planned = resize::plan(4000, 3000, spec, 1.0);
                assert!((1..=resize::MAX_SIDE).contains(&planned.width));
                assert!((1..=resize::MAX_SIDE).contains(&planned.height));
            }
            Op::Crop(spec) => {
                let rect = geometry::crop_rect(4000, 3000, spec);
                assert!(rect.x + rect.width <= 4000 && rect.y + rect.height <= 3000);
            }
            _ => {}
        }
    }
});
