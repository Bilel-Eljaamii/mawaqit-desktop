/** Hostile CSS contract for the dialog layer. Every dialog hides via the
 * `hidden` attribute, whose UA `[hidden] { display: none }` is beaten by
 * any author `display:` declared on the dialog's own ID selector — that is
 * exactly the 2026-10-03 announce-dialog defect: the inbox sat fixed and
 * centered on z-index 1001 and the ✕ button only set `hidden`, which the
 * ID rule overrode, so it could never be closed. A rule keyed on a dialog
 * id may declare `display:` only behind `:not([hidden])`. */
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
const DIALOG_IDS = [
  "overlay",
  "mosque-dialog",
  "settings-dialog",
  "announce-dialog",
  "notify-dialog",
] as const;

interface CssRule {
  selector: string;
  body: string;
}

/** Flat rule parser (styles.css deliberately has no @-rules). */
function styleRules(css: string): CssRule[] {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const rules: CssRule[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(stripped)) !== null) {
    rules.push({ selector: m[1].trim(), body: m[2] });
  }
  return rules;
}

/** The subject compound of one selector: what the declarations actually
 * style (`body #x` → `#x`; `#x .row` → `.row`, which cannot unhide `#x`). */
function subjectCompound(selector: string): string {
  const compounds = selector.trim().split(/[\s>+~]+/);
  return compounds[compounds.length - 1] ?? "";
}

describe("dialog layer CSS contract", () => {
  // Vitest runs from the project root; resolve against it, not import.meta.
  const css = readFileSync("src/styles.css", "utf8");

  it("never declares display on a dialog id outside :not([hidden])", () => {
    const offenders: string[] = [];
    for (const rule of styleRules(css)) {
      for (const raw of rule.selector.split(",")) {
        const subject = subjectCompound(raw);
        const dialogId = DIALOG_IDS.find((id) => subject.includes(`#${id}`));
        if (!dialogId) continue;
        const setsDisplay = /(^|[^-])display\s*:/.test(rule.body);
        const hiddenGated = subject.includes(":not([hidden])");
        if (setsDisplay && !hiddenGated) {
          offenders.push(`${raw.trim()} { … display: … }`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("the announce dialog keeps its flex layout while visible", () => {
    const visible = styleRules(css).find((r) =>
      r.selector.split(",").some((s) => subjectCompound(s) === "#announce-dialog:not([hidden])"),
    );
    expect(visible).toBeDefined();
    expect(visible?.body).toContain("display: flex");
    expect(visible?.body).toContain("flex-direction: column");
  });
});
