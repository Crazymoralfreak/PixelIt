//! Recipes: named pipelines the user saves and reuses.

use std::fs;
use std::path::Path;

use pixelit_core::pipeline::{
    Destination, Effort, Filter, FilterKind, MetadataMode, Op, OutputFormat, Resize, ResizeMode,
    Step, Target,
};
use pixelit_core::{Error, ImageFormat, Pipeline, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::store;

/// Version of `recipes.json`.
pub const RECIPES_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pipeline: Pipeline,
    /// Shipped with the application; its name comes from the translations.
    pub builtin: bool,
    pub pinned: bool,
    /// Unix time of the last change, in seconds.
    pub updated: i64,
}

fn builtin(id: &str, steps: Vec<Op>, tune: impl FnOnce(&mut Pipeline)) -> Recipe {
    let mut pipeline = Pipeline {
        steps: steps
            .into_iter()
            .enumerate()
            .map(|(index, op)| Step::new(&format!("s{}", index + 1), op))
            .collect(),
        ..Pipeline::default()
    };
    pipeline.output.destination = Destination::Subfolder {
        name: String::from("PixelIt"),
    };
    tune(&mut pipeline);
    Recipe {
        id: format!("builtin.{id}"),
        name: String::new(),
        description: String::new(),
        pipeline,
        builtin: true,
        pinned: false,
        updated: 0,
    }
}

fn fit(width: u32, height: u32) -> Op {
    Op::Resize(Resize {
        mode: ResizeMode::Fit,
        width,
        height,
        ..Resize::default()
    })
}

/// Recipes shipped with the application.
pub fn builtins() -> Vec<Recipe> {
    vec![
        builtin("web", vec![fit(1920, 1920)], |pipeline| {
            pipeline.output.format = OutputFormat::Auto;
            pipeline.output.auto_formats = vec![ImageFormat::Webp, ImageFormat::Avif];
            pipeline.output.quality = 80;
            pipeline.output.metadata.mode = MetadataMode::Strip;
        }),
        builtin("smallest", Vec::new(), |pipeline| {
            pipeline.output.format = OutputFormat::Auto;
            pipeline.output.target = Target::Quality { score: 75.0 };
            pipeline.output.effort = Effort::Max;
        }),
        builtin("email", vec![fit(1600, 1600)], |pipeline| {
            pipeline.output.format = OutputFormat::Jpeg;
            pipeline.output.target = Target::Bytes { bytes: 500 * 1024 };
        }),
        builtin("jpeg", Vec::new(), |pipeline| {
            pipeline.output.format = OutputFormat::Jpeg;
            pipeline.output.quality = 90;
        }),
        builtin("png", Vec::new(), |pipeline| {
            pipeline.output.format = OutputFormat::Png;
            pipeline.output.effort = Effort::Max;
        }),
        builtin("clean", Vec::new(), |pipeline| {
            pipeline.output.metadata.mode = MetadataMode::Strip;
        }),
        builtin(
            "thumbnail",
            vec![
                fit(400, 400),
                Op::Filter(Filter {
                    kind: FilterKind::Sharpen,
                    amount: 30.0,
                    radius: 0.8,
                }),
            ],
            |pipeline| {
                pipeline.output.format = OutputFormat::Webp;
                pipeline.output.naming = String::from("{name}_thumb");
            },
        ),
    ]
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Recipes {
    /// Recipes of the user. Built-in ones are added when listing.
    pub recipes: Vec<Recipe>,
}

impl Recipes {
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.recipes(), RECIPES_VERSION, store::no_migration)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.recipes(), RECIPES_VERSION, self)
    }

    /// The user's recipes followed by the built-in ones.
    pub fn all(&self) -> Vec<Recipe> {
        let mut all = self.recipes.clone();
        all.extend(builtins());
        all
    }

    pub fn get(&self, id: &str) -> Option<Recipe> {
        self.recipes
            .iter()
            .find(|recipe| recipe.id == id)
            .cloned()
            .or_else(|| builtins().into_iter().find(|recipe| recipe.id == id))
    }

    /// Finds a recipe by id, or by name without regard to letter case.
    pub fn find(&self, key: &str) -> Option<Recipe> {
        self.get(key).or_else(|| {
            let wanted = key.trim().to_lowercase();
            self.all().into_iter().find(|recipe| {
                recipe.name.to_lowercase() == wanted
                    || recipe.id.strip_prefix("builtin.") == Some(wanted.as_str())
            })
        })
    }

    /// Adds a recipe or replaces the one with the same id. An empty id makes
    /// a new recipe. Returns the stored recipe.
    pub fn upsert(&mut self, mut recipe: Recipe) -> Recipe {
        recipe.builtin = false;
        recipe.updated = chrono::Utc::now().timestamp();
        if recipe.id.is_empty() || recipe.id.starts_with("builtin.") {
            recipe.id = format!("r{:x}{:06x}", recipe.updated, fastrand::u32(..0x00FF_FFFF));
        }
        match self.recipes.iter_mut().find(|known| known.id == recipe.id) {
            Some(known) => *known = recipe.clone(),
            None => self.recipes.push(recipe.clone()),
        }
        recipe
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.recipes.len();
        self.recipes.retain(|recipe| recipe.id != id);
        self.recipes.len() != before
    }
}

/// Writes a recipe into a file that can be shared.
pub fn export(recipe: &Recipe, path: &Path) -> Result<()> {
    store::save_json(path, RECIPES_VERSION, recipe)
}

/// Reads a shared recipe file. A bare pipeline file is accepted as well.
pub fn import(path: &Path) -> Result<Recipe> {
    let text = fs::read_to_string(path)
        .map_err(|error| Error::from(error).with("path", path.display()))?;
    let name = path
        .file_stem()
        .map_or_default(|stem| stem.to_string_lossy().into_owned());
    let value: serde_json::Value = serde_json::from_str(&text)?;
    let mut recipe = if value.get("data").is_some() {
        store::load_json::<Recipe>(path, RECIPES_VERSION, store::no_migration)?
    } else {
        Recipe {
            pipeline: Pipeline::from_json(&text)?,
            ..Recipe::default()
        }
    };
    if recipe.pipeline.version > pixelit_core::pipeline::PIPELINE_VERSION {
        return Err(Error::new("newerFile").with("version", recipe.pipeline.version));
    }
    if recipe.name.trim().is_empty() {
        recipe.name = name;
    }
    recipe.id.clear();
    recipe.builtin = false;
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_have_unique_ids_and_survive_json() {
        let all = builtins();
        let mut ids: Vec<&str> = all.iter().map(|recipe| recipe.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for recipe in &all {
            let json = serde_json::to_string(recipe).unwrap();
            let back: Recipe = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, recipe);
        }
    }

    #[test]
    fn upsert_get_remove() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let mut recipes = Recipes::load(&paths);
        let stored = recipes.upsert(Recipe {
            name: "Мой рецепт".into(),
            ..Recipe::default()
        });
        assert!(!stored.id.is_empty());
        let mut changed = stored.clone();
        changed.name = "Renamed".into();
        recipes.upsert(changed);
        assert_eq!(recipes.recipes.len(), 1);
        recipes.save(&paths).unwrap();

        let loaded = Recipes::load(&paths);
        assert_eq!(loaded.get(&stored.id).unwrap().name, "Renamed");
        assert_eq!(loaded.find("renamed").unwrap().id, stored.id);
        assert!(loaded.find("web").unwrap().builtin);
        assert!(loaded.get("builtin.web").is_some());
        assert_eq!(loaded.all().len(), 1 + builtins().len());

        // Saving a changed built-in recipe makes a copy.
        let copy = recipes.upsert(loaded.get("builtin.web").unwrap());
        assert!(!copy.builtin);
        assert!(!copy.id.starts_with("builtin."));
        assert!(recipes.remove(&copy.id));
        assert!(!recipes.remove(&copy.id));
    }

    #[test]
    fn export_and_import() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Web.pixelit-recipe.json");
        let recipe = builtins().remove(0);
        export(&recipe, &path).unwrap();
        let back = import(&path).unwrap();
        assert_eq!(back.pipeline, recipe.pipeline);
        assert_eq!(back.name, "Web.pixelit-recipe");
        assert!(!back.builtin);

        let bare = folder.path().join("bare.json");
        fs::write(&bare, serde_json::to_string(&recipe.pipeline).unwrap()).unwrap();
        assert_eq!(import(&bare).unwrap().pipeline, recipe.pipeline);

        let newer = folder.path().join("newer.json");
        fs::write(&newer, r#"{"version":99,"steps":[]}"#).unwrap();
        assert_eq!(import(&newer).unwrap_err().code, "newerFile");
    }
}
