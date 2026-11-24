// Recipes (saved pipelines) and the folders that are processed by a recipe
// as soon as a picture appears in them.

import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { OutputEditor } from "../components/OutputEditor";
import { PathField } from "../components/PathField";
import { StepsEditor } from "../components/StepsEditor";
import { useT } from "../i18n";
import { newId } from "../lib/format";
import { newPipeline, recipeName } from "../lib/pipeline";
import type { Recipe, WatchRule } from "../types";
import {
  Button,
  Checkbox,
  EmptyState,
  Field,
  Modal,
  NumberInput,
  PageFrame,
  Section,
  Select,
  Tabs,
  Tag,
  TextInput,
} from "../ui";

function RecipeDialog({
  recipe,
  onSave,
  onClose,
}: {
  recipe: Recipe;
  onSave: (recipe: Recipe) => void;
  onClose: () => void;
}) {
  const { t } = useT();
  const [draft, setDraft] = useState(recipe);
  return (
    <Modal
      title={recipe.id === "" ? t("recipes.newTitle") : t("recipes.editTitle")}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={draft.name.trim() === ""} onClick={() => onSave(draft)}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field label={t("recipes.name")}>
        <TextInput value={draft.name} onChange={(name) => setDraft({ ...draft, name })} wide autoFocus />
      </Field>
      <Field label={t("recipes.description")}>
        <TextInput value={draft.description} onChange={(description) => setDraft({ ...draft, description })} wide />
      </Field>
      <Field label="">
        <Checkbox
          checked={draft.pinned}
          onChange={(pinned) => setDraft({ ...draft, pinned })}
          label={t("recipes.pinned")}
        />
      </Field>
      <Section title={t("batch.steps")}>
        <StepsEditor
          steps={draft.pipeline.steps}
          onChange={(steps) => setDraft({ ...draft, pipeline: { ...draft.pipeline, steps } })}
        />
      </Section>
      <Section title={t("batch.output")} collapsible defaultOpen={false}>
        <OutputEditor
          value={draft.pipeline.output}
          onChange={(output) => setDraft({ ...draft, pipeline: { ...draft.pipeline, output } })}
        />
      </Section>
    </Modal>
  );
}

function WatchDialog({
  rule,
  recipes,
  onSave,
  onClose,
}: {
  rule: WatchRule;
  recipes: Recipe[];
  onSave: (rule: WatchRule) => void;
  onClose: () => void;
}) {
  const { t, td } = useT();
  const [draft, setDraft] = useState(rule);
  const complete = draft.folder.trim() !== "" && draft.recipeId !== "";
  return (
    <Modal
      title={rule.id === "" ? t("watch.newTitle") : t("watch.editTitle")}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={!complete} onClick={() => onSave(draft)}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field label={t("recipes.name")}>
        <TextInput value={draft.name} onChange={(name) => setDraft({ ...draft, name })} wide />
      </Field>
      <Field label={t("watch.folder")}>
        <PathField kind="folder" value={draft.folder} onChange={(folder) => setDraft({ ...draft, folder })} />
      </Field>
      <Field label="">
        <Checkbox
          checked={draft.recursive}
          onChange={(recursive) => setDraft({ ...draft, recursive })}
          label={t("batch.recursive")}
        />
      </Field>
      <Field label={t("watch.recipe")} hint={t("watch.recipeHint")}>
        <Select
          value={draft.recipeId}
          options={[
            { value: "", label: t("batch.chooseRecipe") },
            ...recipes.map((recipe) => ({ value: recipe.id, label: recipeName(recipe, td) })),
          ]}
          onChange={(recipeId) => setDraft({ ...draft, recipeId })}
        />
      </Field>
      <Field label={t("watch.settle")} hint={t("watch.settleHint")}>
        <NumberInput
          value={draft.settleMs}
          onChange={(settleMs) => setDraft({ ...draft, settleMs })}
          min={200}
          max={60000}
          step={100}
          suffix="ms"
          wide
        />
      </Field>
      <Field label="">
        <Checkbox
          checked={draft.enabled}
          onChange={(enabled) => setDraft({ ...draft, enabled })}
          label={t("watch.enabled")}
        />
      </Field>
    </Modal>
  );
}

export default function RecipesPage() {
  const { t, td } = useT();
  const { settings, recipes, refreshRecipes, setBatchPipeline, navigate, confirm, notify, showError } = useApp();
  const [tab, setTab] = useState<"recipes" | "watch">("recipes");
  const [editing, setEditing] = useState<Recipe | null>(null);
  const [rules, setRules] = useState<WatchRule[]>([]);
  const [editingRule, setEditingRule] = useState<WatchRule | null>(null);

  const refreshRules = useCallback(() => {
    api.watchList().then(setRules).catch(showError);
  }, [showError]);

  useEffect(refreshRules, [refreshRules]);

  const blank = (): Recipe => ({
    id: "",
    name: "",
    description: "",
    pipeline: newPipeline(settings.output),
    builtin: false,
    pinned: false,
    updated: 0,
  });

  const saveRecipe = async (recipe: Recipe) => {
    try {
      await api.recipeSave(recipe);
      setEditing(null);
      refreshRecipes();
    } catch (error) {
      showError(error);
    }
  };

  const removeRecipe = async (recipe: Recipe) => {
    const agreed = await confirm({
      title: t("recipes.removeTitle"),
      text: t("recipes.removeText", { name: recipe.name }),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (agreed) {
      api.recipeRemove(recipe.id).then(refreshRecipes).then(refreshRules).catch(showError);
    }
  };

  const exportRecipe = async (recipe: Recipe) => {
    const path = await api.pickSave(`${recipeName(recipe, td)}.pixelit.json`, t("recipes.fileKind"), ["json"]);
    if (path) {
      api
        .recipeExport(recipe.id, path)
        .then(() => notify(t("recipes.exported")))
        .catch(showError);
    }
  };

  const importRecipe = async () => {
    const path = await api.pickFile(t("recipes.fileKind"), ["json"]);
    if (path) {
      api
        .recipeImport(path)
        .then((recipe) => {
          refreshRecipes();
          notify(t("recipes.saved", { name: recipe.name }));
        })
        .catch(showError);
    }
  };

  const sendToBatch = (recipe: Recipe) => {
    setBatchPipeline({
      ...recipe.pipeline,
      steps: recipe.pipeline.steps.map((step) => ({ ...step, id: newId() })),
    });
    navigate("batch");
  };

  const saveRule = async (rule: WatchRule) => {
    try {
      await api.watchSave(rule);
      setEditingRule(null);
      refreshRules();
    } catch (error) {
      showError(error);
    }
  };

  const ordered = [...recipes].sort((a, b) => Number(b.pinned) - Number(a.pinned));

  return (
    <PageFrame
      title={t("nav.recipes")}
      actions={
        tab === "recipes" ? (
          <>
            <Button onClick={() => void importRecipe()}>{t("recipes.import")}</Button>
            <Button variant="primary" onClick={() => setEditing(blank())}>
              {t("recipes.new")}
            </Button>
          </>
        ) : (
          <Button
            variant="primary"
            onClick={() =>
              setEditingRule({
                id: "",
                name: "",
                folder: "",
                recursive: false,
                recipeId: "",
                enabled: true,
                settleMs: 1500,
              })
            }
          >
            {t("watch.new")}
          </Button>
        )
      }
    >
      <Tabs
        value={tab}
        options={[
          { value: "recipes", label: t("recipes.tab") },
          { value: "watch", label: t("watch.tab") },
        ]}
        onChange={setTab}
      />
      {tab === "recipes" ? (
        <div className="cards">
          {ordered.map((recipe) => (
            <div key={recipe.id} className="card">
              <div className="card-title">
                {recipeName(recipe, td)}
                {recipe.builtin ? <Tag>{t("recipes.builtinTag")}</Tag> : null}
                {recipe.pinned ? <Tag tone="ok">{t("recipes.pinnedTag")}</Tag> : null}
              </div>
              <div className="card-text">
                {recipe.builtin
                  ? td(`recipes.builtinText.${recipe.id.replace(/^builtin\./, "")}`, undefined, "")
                  : recipe.description}
              </div>
              <div className="card-text card-steps">
                {recipe.pipeline.steps.map((step) => td(`ops.${step.op.type}`)).join(" → ") || t("recipes.onlyOutput")}
              </div>
              <div className="card-actions">
                <Button small variant="primary" onClick={() => sendToBatch(recipe)}>
                  {t("recipes.use")}
                </Button>
                <Button
                  small
                  onClick={() =>
                    setEditing(
                      recipe.builtin
                        ? { ...recipe, id: "", name: recipeName(recipe, td), builtin: false }
                        : recipe,
                    )
                  }
                >
                  {recipe.builtin ? t("recipes.copy") : t("common.edit")}
                </Button>
                <Button small onClick={() => void exportRecipe(recipe)}>
                  {t("recipes.export")}
                </Button>
                {recipe.builtin ? null : (
                  <Button small onClick={() => void removeRecipe(recipe)}>
                    {t("common.delete")}
                  </Button>
                )}
              </div>
            </div>
          ))}
        </div>
      ) : rules.length === 0 ? (
        <EmptyState title={t("watch.emptyTitle")} text={t("watch.emptyText")} />
      ) : (
        <div className="cards">
          {rules.map((rule) => {
            const recipe = recipes.find((item) => item.id === rule.recipeId);
            return (
              <div key={rule.id} className="card">
                <div className="card-title">
                  {rule.name || rule.folder}
                  <Tag tone={rule.enabled ? "ok" : "plain"}>{rule.enabled ? t("watch.on") : t("watch.off")}</Tag>
                </div>
                <div className="card-text" title={rule.folder}>
                  {rule.folder}
                </div>
                <div className="card-text">
                  {recipe ? recipeName(recipe, td) : <Tag tone="danger">{t("watch.recipeMissing")}</Tag>}
                </div>
                <div className="card-actions">
                  <Button small onClick={() => void saveRule({ ...rule, enabled: !rule.enabled })}>
                    {rule.enabled ? t("watch.pause") : t("watch.resume")}
                  </Button>
                  <Button small onClick={() => setEditingRule(rule)}>
                    {t("common.edit")}
                  </Button>
                  <Button small onClick={() => api.watchRemove(rule.id).then(refreshRules).catch(showError)}>
                    {t("common.delete")}
                  </Button>
                </div>
              </div>
            );
          })}
        </div>
      )}
      {editing ? <RecipeDialog recipe={editing} onSave={(recipe) => void saveRecipe(recipe)} onClose={() => setEditing(null)} /> : null}
      {editingRule ? (
        <WatchDialog
          rule={editingRule}
          recipes={recipes}
          onSave={(rule) => void saveRule(rule)}
          onClose={() => setEditingRule(null)}
        />
      ) : null}
    </PageFrame>
  );
}
