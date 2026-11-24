// A path typed by hand or chosen in a dialog of the system.

import { api } from "../api";
import { useT } from "../i18n";
import { Button, TextInput } from "../ui";

interface PathFieldProps {
  value: string;
  onChange: (path: string) => void;
  /** What the dialog chooses: a folder, a picture or a file of the given kind. */
  kind: "folder" | "picture" | "file";
  /** Name and extensions of the files the dialog shows, for `file`. */
  filter?: { name: string; extensions: string[] };
  placeholder?: string;
  disabled?: boolean;
}

export function PathField({ value, onChange, kind, filter, placeholder, disabled }: PathFieldProps) {
  const { t } = useT();

  const browse = async () => {
    let chosen: string | null | undefined;
    switch (kind) {
      case "folder":
        chosen = await api.pickFolder(value || undefined);
        break;
      case "picture":
        chosen = (await api.pickFiles(false))[0];
        break;
      case "file":
        chosen = await api.pickFile(filter?.name ?? "", filter?.extensions ?? []);
        break;
      default: {
        const unreachable: never = kind;
        throw new Error(`Unknown path kind: ${String(unreachable)}`);
      }
    }
    if (chosen) {
      onChange(chosen);
    }
  };

  return (
    <span className="path-field">
      <TextInput value={value} onChange={onChange} wide placeholder={placeholder} disabled={disabled} />
      <Button onClick={() => void browse()} disabled={disabled}>
        {t("common.browse")}
      </Button>
    </span>
  );
}
