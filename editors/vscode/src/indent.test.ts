// The indent model is pure text-in / levels-out, so it is tested without a
// VS Code fixture. Run with `npm test`.
//
// These are the same cases as the IntelliJ plugin's `SauleIndentModelTest` and
// the Neovim `tests/indent_spec.lua` — the three implementations are ports of
// each other, so they share a test corpus. Add a case to one, add it to all.

import { strict as assert } from "node:assert";
import { test } from "node:test";

import {
  SauleIndent,
  indentForLine,
  keywordTypedAt,
} from "./indent";

test("class body and methods", () => {
  assertRoundTrips(`
    export class Warrior extends Entity
      local health: integer

      fn init(name: string)
        self.super(name)
      end
    end
  `);
});

test("if elseif else", () => {
  assertRoundTrips(`
    fn f()
      if a then
        x()
      elseif b then
        y()
      else
        z()
      end
    end
  `);
});

test("loops and repeat until", () => {
  assertRoundTrips(`
    fn f()
      for i: integer in {1, 2, 3} do
        printf("hit %d\\n", i)
      end

      while cond do
        step()
      end

      repeat
        step()
      until done
    end
  `);
});

test("match arms stay at body level", () => {
  assertRoundTrips(`
    fn f()
      return match self.health
        case 0 then false
        case hp when hp < 0 then false
        case _ then true
      end
    end
  `);
});

test("match arm with a block body indents its statements", () => {
  assertRoundTrips(`
    fn f()
      match x
        case 1 then
          a()
          b()
        case 2 then
          c()
        case _ then nothing()
      end
    end
  `);
});

test("interface signatures have no end", () => {
  assertRoundTrips(`
    export interface Drawable
      fn draw(target: any)
      fn bounds() -> table
    end

    class Sprite implements Drawable
      fn draw(target: any)
        target.blit(self)
      end
    end
  `);
});

test("enum variants then methods", () => {
  assertRoundTrips(`
    enum Color
      Red,
      Green,

      fn name() -> string
        return "?"
      end
    end
  `);
});

test("try catch", () => {
  assertRoundTrips(`
    fn f()
      try
        risky()
      catch e: any
        log(e)
      end
    end
  `);
});

test("lambda block body", () => {
  assertRoundTrips(`
    local handler = fn(x: integer)
      return x + 1
    end
  `);
});

test("a fn type annotation is not a block", () => {
  assertRoundTrips(`
    fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
      local out: table<U> = {}

      for item: T in items do
        out[#out + 1] = f(item)
      end

      return out
    end

    local lengths = map({"a", "bb"}, s => #s)
  `);
});

test("a fn type in a local annotation is not a block", () => {
  assertRoundTrips(`
    local double: fn(integer) -> integer = fn(x: integer) -> integer
      return x * 2
    end

    println(double(2))
  `);
});

test("a new line after fn-typed signatures starts at column zero", () => {
  // The reported bug: pressing Enter below the last statement of a file whose
  // functions take `fn(T) -> U` callbacks produced two tabs and sixteen spaces.
  const text =
    "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n" +
    "  return items\n" +
    "end\n" +
    "\n" +
    "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n" +
    "  return items\n" +
    "end\n" +
    "\n" +
    'local lengths = map({"a"}, s => #s)\n' +
    "\n";
  assertIndent(SauleIndent.ZERO, indentOfLine(text, 9));
});

test("an anonymous fn argument is still a block", () => {
  // The counter-case the type-position rule must not break: `fn` after a comma
  // opens a real body.
  assertIndent(
    new SauleIndent(1, 1),
    indentOfLine("map(xs, fn(x: integer) -> integer\n\n", 1),
  );
});

test("keywords inside strings and comments are ignored", () => {
  assertRoundTrips(`
    fn f()
      -- end
      local s: string = "end end end"
      --[[ class Foo ]]
      return s
    end
  `);
});

test("inside a block comment the enclosing block's indent is used", () => {
  const text = "class A\n  --[[ text\n\n  ]]\nend\n";
  assertIndent(new SauleIndent(1, 0), indentOfLine(text, 2));
});

test("open bracket adds a continuation level", () => {
  const text = "foo(\n  a,\n  b,\n)\n";
  assertIndent(new SauleIndent(0, 0), indentOfLine(text, 0));
  assertIndent(new SauleIndent(0, 1), indentOfLine(text, 1));
  assertIndent(new SauleIndent(0, 1), indentOfLine(text, 2));
  assertIndent(new SauleIndent(0, 0), indentOfLine(text, 3));
});

test("a blank line takes the enclosing block's indent", () => {
  const text = "class A\n\n  fn f()\n\nend\n";
  assertIndent(new SauleIndent(1, 0), indentOfLine(text, 1));
  // Inside `fn f()`, still open at this point.
  assertIndent(new SauleIndent(2, 0), indentOfLine(text, 3));
});

test("a closer typed at the body indent still resolves one level out", () => {
  // What the editor sees mid-keystroke: Enter has indented the line to the body
  // level and the closer has just been typed into it. The answer must not
  // depend on the whitespace already there.
  for (const opener of [
    "fn f()",
    "if a then",
    "while a do",
    "for i in x do",
    "try",
    "match x",
  ]) {
    const text = `class A\n  ${opener}\n    end\n`;
    assertIndent(new SauleIndent(1, 0), indentOfLine(text, 2), opener);
  }
  assertIndent(
    new SauleIndent(1, 0),
    indentOfLine("class A\n  repeat\n    until done\n", 2),
  );
  assertIndent(
    new SauleIndent(1, 0),
    indentOfLine("class A\n  if a then\n    else\n", 2),
  );
  assertIndent(
    new SauleIndent(1, 0),
    indentOfLine("class A\n  try\n    catch e: any\n", 2),
  );
});

test("a closer that turns out to be an identifier keeps the body indent", () => {
  // `end` dedents as it is typed, so `endless` has to put it back.
  assertIndent(
    new SauleIndent(2, 0),
    indentOfLine("class A\n  fn f()\n    endless()\n", 2),
  );
});

test("keywordTypedAt fires only on a bare closer", () => {
  assert.ok(typedAt("fn f()\n  end"));
  assert.ok(typedAt("fn f()\n  else"));
  assert.ok(typedAt("repeat\n  until"));
  assert.ok(typedAt("match x\n  case"));
  // One character past a closer: the indent has to be restored.
  assert.ok(typedAt("fn f()\n  endl"));
  // Half-typed, mid-expression, or not a keyword at all.
  assert.ok(!typedAt("fn f()\n  en"));
  assert.ok(!typedAt("fn f()\n  x = end"));
  assert.ok(!typedAt("fn f()\n  endles"));
  assert.ok(!typedAt("fn f()\n  "));
});

test("render uses tabs when the editor asks for them", () => {
  assert.equal(new SauleIndent(2, 0).render(2, true), "    ");
  assert.equal(new SauleIndent(1, 1).render(2, true), "      ");
  // Tabs to indent, spaces to align.
  assert.equal(new SauleIndent(2, 0).render(2, false), "\t\t");
  assert.equal(new SauleIndent(1, 1).render(2, false), "\t    ");
});

// ── helpers ─────────────────────────────────────────────────────────────────

function typedAt(text: string): boolean {
  return keywordTypedAt(text, text.length);
}

function assertIndent(
  expected: SauleIndent,
  actual: SauleIndent,
  message?: string,
): void {
  assert.deepEqual(
    { blocks: actual.blocks, continuations: actual.continuations },
    { blocks: expected.blocks, continuations: expected.continuations },
    message,
  );
}

function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\n") starts.push(i + 1);
  }
  return starts;
}

function indentOfLine(text: string, line: number): SauleIndent {
  const start = lineStarts(text)[line];
  let end = start;
  while (end < text.length && text[end] !== "\n") end++;
  return indentForLine(text, start, end);
}

/** Strip the leading indentation the template literal is written at. */
function trimIndent(sample: string): string {
  const lines = sample.split("\n").filter((l, i, all) => {
    const edge = i === 0 || i === all.length - 1;
    return !(edge && l.trim() === "");
  });
  const common = Math.min(
    ...lines.filter((l) => l.trim() !== "").map((l) => l.length - l.trimStart().length),
  );
  return lines.map((l) => l.slice(common)).join("\n").trimEnd() + "\n";
}

/**
 * Asserts that re-deriving each line's indent reproduces the sample, i.e. that
 * a file already in canonical form is a fixed point of the model.
 */
function assertRoundTrips(sample: string): void {
  const text = trimIndent(sample);
  const starts = lineStarts(text);
  text.split("\n").forEach((line, i) => {
    if (i >= starts.length || line.trim() === "") return;
    const expected = (line.length - line.trimStart().length) / 2;
    assertIndent(
      new SauleIndent(expected, 0),
      indentOfLine(text, i),
      `line ${i + 1}: "${line}"`,
    );
  });
}
