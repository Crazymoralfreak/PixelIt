// Shared UI primitives. Feature screens build on these and on the classes in styles.css.

import {
  useEffect,
  useState,
  type ButtonHTMLAttributes,
  type InputHTMLAttributes,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";

export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

type ButtonVariant = "default" | "primary" | "danger" | "ghost";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  small?: boolean;
}

export function Button({ variant = "default", small, className, type, ...rest }: ButtonProps) {
  return (
    <button
      type={type ?? "button"}
      className={cx("btn", `btn-${variant}`, small && "btn-small", className)}
      {...rest}
    />
  );
}

interface TextInputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange"> {
  value: string;
  onChange: (value: string) => void;
  invalid?: boolean;
  wide?: boolean;
}

export function TextInput({ value, onChange, invalid, wide, className, ...rest }: TextInputProps) {
  return (
    <input
      type="text"
      className={cx("input", wide && "input-wide", invalid && "input-invalid", className)}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      {...rest}
    />
  );
}

interface TextAreaProps {
  value: string;
  onChange: (value: string) => void;
  rows?: number;
  placeholder?: string;
  disabled?: boolean;
  onKeyDown?: (event: ReactKeyboardEvent<HTMLTextAreaElement>) => void;
}

export function TextArea({ value, onChange, rows = 3, placeholder, disabled, onKeyDown }: TextAreaProps) {
  return (
    <textarea
      className="input textarea"
      value={value}
      rows={rows}
      placeholder={placeholder}
      disabled={disabled}
      onKeyDown={onKeyDown}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

interface NumberInputProps {
  value: number;
  /** Called with every number typed; values out of range are clamped on blur. */
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  /** Short unit shown after the field, e.g. "px". */
  suffix?: string;
  wide?: boolean;
  title?: string;
}

export function NumberInput({ value, onChange, min, max, step = 1, disabled, suffix, wide, title }: NumberInputProps) {
  const [text, setText] = useState(String(value));

  useEffect(() => {
    setText((current) => (Number(current) === value && current.trim() !== "" ? current : String(value)));
  }, [value]);

  const clamp = (number: number) => Math.min(max ?? Infinity, Math.max(min ?? -Infinity, number));

  const input = (
    <input
      type="number"
      className={cx("input", "input-number", wide && "input-number-wide")}
      value={text}
      min={min}
      max={max}
      step={step}
      disabled={disabled}
      title={title}
      onChange={(event) => {
        const next = event.target.value;
        setText(next);
        const parsed = Number(next);
        if (next.trim() !== "" && Number.isFinite(parsed)) {
          onChange(clamp(Number.isInteger(step) ? Math.trunc(parsed) : parsed));
        }
      }}
      onBlur={() => setText(String(value))}
    />
  );
  if (!suffix) {
    return input;
  }
  return (
    <span className="input-with-suffix">
      {input}
      <span className="input-suffix">{suffix}</span>
    </span>
  );
}

interface SliderProps {
  value: number;
  onChange: (value: number) => void;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  /** Value a double click returns to. */
  reset?: number;
}

/** A range with the number beside it. */
export function Slider({ value, onChange, min, max, step = 1, disabled, reset }: SliderProps) {
  return (
    <span className="slider">
      <input
        type="range"
        className="slider-range"
        value={value}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onChange={(event) => onChange(Number(event.target.value))}
        onDoubleClick={() => reset !== undefined && onChange(reset)}
      />
      <NumberInput value={value} onChange={onChange} min={min} max={max} step={step} disabled={disabled} />
    </span>
  );
}

export interface Option<T extends string> {
  value: T;
  label: string;
  disabled?: boolean;
}

interface SelectProps<T extends string> {
  value: T;
  options: ReadonlyArray<Option<T>>;
  onChange: (value: T) => void;
  disabled?: boolean;
  title?: string;
}

export function Select<T extends string>({ value, options, onChange, disabled, title }: SelectProps<T>) {
  return (
    <select
      className="input select"
      value={value}
      disabled={disabled}
      title={title}
      onChange={(event) => onChange(event.target.value as T)}
    >
      {options.map((option) => (
        <option key={option.value} value={option.value} disabled={option.disabled}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

/** Row of mutually exclusive buttons. */
export function Segmented<T extends string>({ value, options, onChange, disabled }: SelectProps<T>) {
  return (
    <div className="segmented" role="radiogroup">
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          className={cx("segmented-item", option.value === value && "segmented-item-active")}
          disabled={disabled || option.disabled}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  disabled?: boolean;
  title?: string;
}

export function Checkbox({ checked, onChange, label, disabled, title }: CheckboxProps) {
  return (
    <label className={cx("checkbox", disabled && "checkbox-disabled")} title={title}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

interface ColorInputProps {
  /** `#rrggbb` or `#rrggbbaa`; the alpha part is kept as it is. */
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}

export function ColorInput({ value, onChange, disabled }: ColorInputProps) {
  const rgb = /^#[0-9a-f]{6}/i.test(value) ? value.slice(0, 7) : "#000000";
  const alpha = value.length === 9 ? value.slice(7) : "";
  return (
    <input
      type="color"
      className="color-input"
      value={rgb}
      disabled={disabled}
      onChange={(event) => onChange(`${event.target.value}${alpha}`)}
    />
  );
}

interface FieldProps {
  label: ReactNode;
  /** Grey explanatory line under the control. */
  hint?: ReactNode;
  /** Red line under the control. */
  error?: ReactNode;
  children: ReactNode;
}

/** Label on the left, control with hint and error on the right. */
export function Field({ label, hint, error, children }: FieldProps) {
  return (
    <div className="field">
      <div className="field-label">{label}</div>
      <div className="field-body">
        <div className="field-control">{children}</div>
        {hint ? <div className="field-hint">{hint}</div> : null}
        {error ? <div className="field-error">{error}</div> : null}
      </div>
    </div>
  );
}

interface ModalProps {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Buttons at the bottom, right aligned. */
  footer?: ReactNode;
  wide?: boolean;
}

export function Modal({ title, onClose, children, footer, wide }: ModalProps) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div className={cx("modal", wide && "modal-wide")} role="dialog" aria-modal="true" aria-label={title}>
        <div className="modal-title">{title}</div>
        <div className="modal-body">{children}</div>
        {footer ? <div className="modal-footer">{footer}</div> : null}
      </div>
    </div>
  );
}

interface EmptyStateProps {
  title: string;
  text?: ReactNode;
  action?: ReactNode;
}

export function EmptyState({ title, text, action }: EmptyStateProps) {
  return (
    <div className="empty-state">
      <div className="empty-state-title">{title}</div>
      {text ? <div className="empty-state-text">{text}</div> : null}
      {action ? <div className="empty-state-action">{action}</div> : null}
    </div>
  );
}

/** Page frame: title row with actions, then scrollable content. */
export function PageFrame({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="page">
      <div className="page-header">
        <h1 className="page-title">{title}</h1>
        <div className="page-actions">{actions}</div>
      </div>
      <div className="page-content">{children}</div>
    </div>
  );
}

/**
 * Titled group of fields inside a page. A collapsible group opens on a click
 * of its title row; while it is closed its content is not rendered and its
 * actions are hidden, so nothing happens out of sight.
 */
export function Section({
  title,
  children,
  collapsible = false,
  defaultOpen = true,
  summary,
  actions,
}: {
  title?: string;
  children: ReactNode;
  collapsible?: boolean;
  defaultOpen?: boolean;
  /** A short state beside the title: a count, the chosen model. It stays visible while the group is closed. */
  summary?: ReactNode;
  actions?: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const shown = !collapsible || !title || open;
  const state = summary ? <span className="section-summary">{summary}</span> : null;
  return (
    <section className={cx("section", !shown && "section-collapsed")}>
      {title ? (
        <div className="section-head">
          <h2 className="section-title">
            {collapsible ? (
              <button
                type="button"
                className="section-toggle"
                aria-expanded={open}
                onClick={() => setOpen((current) => !current)}
              >
                <span className={cx("section-mark", open && "section-mark-open")} aria-hidden="true" />
                <span className="section-name">{title}</span>
                {state}
              </button>
            ) : (
              <>
                <span className="section-name">{title}</span>
                {state}
              </>
            )}
          </h2>
          {actions && shown ? <div className="section-actions">{actions}</div> : null}
        </div>
      ) : null}
      {shown ? children : null}
    </section>
  );
}

/** Row of tabs; the content is rendered by the caller. */
export function Tabs<T extends string>({ value, options, onChange }: SelectProps<T>) {
  return (
    <div className="tabs" role="tablist">
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="tab"
          aria-selected={option.value === value}
          className={cx("tab", option.value === value && "tab-active")}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** A bar filled to `done / total`; without a total it shows activity. */
export function Progress({ done, total }: { done: number; total: number }) {
  const share = total > 0 ? Math.min(1, done / total) : 0;
  return (
    <div className={cx("progress", total <= 0 && "progress-busy")} role="progressbar">
      <div className="progress-fill" style={{ width: `${share * 100}%` }} />
    </div>
  );
}

/** A small label: a state, a format, a count. */
export function Tag({ children, tone = "plain" }: { children: ReactNode; tone?: "plain" | "ok" | "warning" | "danger" }) {
  return <span className={cx("tag", `tag-${tone}`)}>{children}</span>;
}
