// About the application: the version, what changed, and the licenses of
// everything PixelIt is built from.

import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { parseChangelog, ReleaseNotes, type Release } from "../components/Changelog";
import { useT } from "../i18n";
import libraries from "../licenses.json";
import { EmptyState, PageFrame, Section, Tabs, Tag, TextInput } from "../ui";

const REPOSITORY = "https://github.com/Crazymoralfreak/PixelIt";

export default function AboutPage() {
  const { t, language } = useT();
  const { info, ai } = useApp();
  const [tab, setTab] = useState<"news" | "licenses">("news");
  const [releases, setReleases] = useState<Release[]>([]);
  const [filter, setFilter] = useState("");

  useEffect(() => {
    api
      .changelog()
      .then((text) => setReleases(parseChangelog(text)))
      .catch(() => undefined);
    // The notes come in the language of the interface.
  }, [language]);

  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    return needle === ""
      ? libraries
      : libraries.filter(
          (library) => library.name.toLowerCase().includes(needle) || library.license.toLowerCase().includes(needle),
        );
  }, [filter]);

  return (
    <PageFrame title={t("nav.about")}>
      <Section>
        <div className="about-head">
          <span className="about-name">PixelIt</span>
          <Tag>{info.version}</Tag>
          {info.portable ? <Tag tone="warning">{t("settings.portable")}</Tag> : null}
        </div>
        <p className="tool-text">{t("about.text")}</p>
        <p className="tool-text">
          {t("about.license")} · <span className="path-text">{REPOSITORY}</span>
        </p>
      </Section>
      <Tabs
        value={tab}
        options={[
          { value: "news", label: t("about.whatsNew") },
          { value: "licenses", label: t("about.licenses") },
        ]}
        onChange={setTab}
      />
      {tab === "news" ? (
        releases.length === 0 ? (
          <EmptyState title={t("about.noNews")} />
        ) : (
          releases.map((release) => (
            <Section key={release.version} title={release.date ? `${release.version} · ${release.date}` : release.version}>
              <ReleaseNotes release={release} />
            </Section>
          ))
        )
      ) : (
        <>
          <Section title={t("about.models")}>
            <p className="tool-text">{t("about.modelsText")}</p>
            <table className="table">
              <tbody>
                {(ai?.components ?? []).map((component) => (
                  <tr key={component.id}>
                    <td>{component.name}</td>
                    <td>{component.license}</td>
                    <td className="path-text">{component.source}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Section>
          <Section
            title={t("about.libraries", { count: libraries.length })}
            actions={<TextInput value={filter} onChange={setFilter} placeholder={t("viewer.filter")} />}
          >
            <table className="table">
              <tbody>
                {shown.map((library) => (
                  <tr key={`${library.name}@${library.version}`}>
                    <td>{library.name}</td>
                    <td>{library.version}</td>
                    <td>{library.license}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Section>
        </>
      )}
    </PageFrame>
  );
}
