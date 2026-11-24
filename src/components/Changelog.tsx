// The change log of the application, as CHANGELOG.md and CHANGELOG.ru.md keep it.

export interface Release {
  version: string;
  date: string;
  lines: string[];
}

/** Splits the file into releases by its `## [1.0.0] - 2026-01-01` headings. */
export function parseChangelog(text: string): Release[] {
  const releases: Release[] = [];
  for (const line of text.split(/\r?\n/)) {
    const heading = /^##\s+\[?([^\]\s]+)\]?(?:\s*[-–]\s*(.+))?$/.exec(line);
    if (heading) {
      releases.push({ version: heading[1], date: heading[2]?.trim() ?? "", lines: [] });
    } else if (releases.length > 0) {
      releases[releases.length - 1].lines.push(line);
    }
  }
  return releases;
}

export function ReleaseNotes({ release }: { release: Release }) {
  return (
    <div className="notes">
      {release.lines.map((line, index) => {
        const text = line.trim();
        if (text === "") {
          return null;
        }
        if (text.startsWith("### ")) {
          return (
            <h3 key={index} className="notes-heading">
              {text.slice(4)}
            </h3>
          );
        }
        if (text.startsWith("- ")) {
          return (
            <div key={index} className="notes-item">
              {text.slice(2)}
            </div>
          );
        }
        return (
          <p key={index} className="notes-text">
            {text}
          </p>
        );
      })}
    </div>
  );
}
