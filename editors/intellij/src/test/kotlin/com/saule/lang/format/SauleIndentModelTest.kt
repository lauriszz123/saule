package com.saule.lang.format

import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The indent model is pure text-in / levels-out, so it is tested without an
 * IDE fixture. Each case is written the way `saule fmt` would print it, and
 * asserts that every line's computed indent matches the one it was written at.
 */
class SauleIndentModelTest {

    @Test
    fun `class body and methods`() = assertRoundTrips(
        """
        export class Warrior extends Entity
          local health: integer

          fn init(name: string)
            self.super(name)
          end
        end
        """
    )

    @Test
    fun `if elseif else`() = assertRoundTrips(
        """
        fn f()
          if a then
            x()
          elseif b then
            y()
          else
            z()
          end
        end
        """
    )

    @Test
    fun `loops and repeat until`() = assertRoundTrips(
        """
        fn f()
          for i: integer in {1, 2, 3} do
            printf("hit %d\n", i)
          end

          while cond do
            step()
          end

          repeat
            step()
          until done
        end
        """
    )

    @Test
    fun `a trailing block is a level of its own`() = assertRoundTrips(
        // `f(a) do … end` is sugar for a call whose last argument is a
        // block-bodied lambda, so its body indents like any other block.
        """
        fn f()
          local screen = Canvas() do
            Panel(title: "Saule UI", spacing: 1) do
              Text("Trailing blocks, drawn.")
            end
          end

          println(screen.render())
        end
        """
    )

    @Test
    fun `a loop header's do opens one block, not two`() = assertRoundTrips(
        // The `do` closing a `for` / `while` header belongs to the loop, which
        // is already open; only a `do` outside a header opens a block of its
        // own. Get that wrong and each loop swallows an extra `end`.
        """
        fn f()
          Row(spacing: 3) do
            for i, name in players do
              while ready(name) do
                Button(name)
              end
            end
          end

          done()
        end
        """
    )

    @Test
    fun `both kinds of do indent their body once`() {
        for (opener in listOf("while x", "for i in xs", "Canvas()", "f(a, b)")) {
            assertEquals(opener, SauleIndent(1, 0), indentOfLine("$opener do\n\n", 1))
            assertEquals(
                opener,
                SauleIndent.ZERO,
                indentOfLine("$opener do\n  step()\nend\n\n", 3),
            )
        }
    }

    @Test
    fun `match arms stay at body level`() = assertRoundTrips(
        """
        fn f()
          return match self.health
            case 0 then false
            case hp when hp < 0 then false
            case _ then true
          end
        end
        """
    )

    @Test
    fun `match arm with a block body indents its statements`() = assertRoundTrips(
        """
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
        """
    )

    @Test
    fun `interface signatures have no end`() = assertRoundTrips(
        """
        export interface Drawable
          fn draw(target: any)
          fn bounds() -> table
        end

        class Sprite implements Drawable
          fn draw(target: any)
            target.blit(self)
          end
        end
        """
    )

    @Test
    fun `enum variants then methods`() = assertRoundTrips(
        """
        enum Color
          Red,
          Green,

          fn name() -> string
            return "?"
          end
        end
        """
    )

    @Test
    fun `try catch`() = assertRoundTrips(
        """
        fn f()
          try
            risky()
          catch e: any
            log(e)
          end
        end
        """
    )

    @Test
    fun `lambda block body`() = assertRoundTrips(
        """
        local handler = fn(x: integer)
          return x + 1
        end
        """
    )

    @Test
    fun `a fn type annotation is not a block`() = assertRoundTrips(
        """
        fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>
          local out: table<U> = {}

          for item: T in items do
            out[#out + 1] = f(item)
          end

          return out
        end

        local lengths = map({"a", "bb"}, s => #s)
        """
    )

    @Test
    fun `a fn type in a local annotation is not a block`() = assertRoundTrips(
        """
        local double: fn(integer) -> integer = fn(x: integer) -> integer
          return x * 2
        end

        println(double(2))
        """
    )

    @Test
    fun `a new line after fn-typed signatures starts at column zero`() {
        // The reported bug: pressing `o` below the last statement of a file
        // whose functions take `fn(T) -> U` callbacks produced two tabs and
        // sixteen spaces. Each `fn` type left a block frame open, and the
        // stray frame also swallowed the `)` that closed the parameter list,
        // so the leftovers accumulated per signature.
        val text =
            "fn map<T, U>(items: table<T>, f: fn(T) -> U) -> table<U>\n" +
                "  return items\n" +
                "end\n" +
                "\n" +
                "fn filter<T>(items: table<T>, p: fn(T) -> boolean) -> table<T>\n" +
                "  return items\n" +
                "end\n" +
                "\n" +
                "local lengths = map({\"a\"}, s => #s)\n" +
                "\n"
        assertEquals(SauleIndent.ZERO, indentOfLine(text, 9))
    }

    @Test
    fun `an anonymous fn argument is still a block`() {
        // The counter-case the type-position rule must not break: `fn` after
        // a comma opens a real body.
        assertEquals(
            SauleIndent(1, 1),
            indentOfLine("map(xs, fn(x: integer) -> integer\n\n", 1),
        )
    }

    @Test
    fun `keywords inside strings and comments are ignored`() = assertRoundTrips(
        """
        fn f()
          -- end
          local s: string = "end end end"
          --[[ class Foo ]]
          return s
        end
        """
    )

    @Test
    fun `inside a block comment the enclosing block's indent is used`() {
        // No attempt at aligning to the comment's own layout: a new line in
        // the middle of `--[[ … ]]` simply lands at the block level.
        val text = "class A\n  --[[ text\n\n  ]]\nend\n"
        assertEquals(SauleIndent(1, 0), indentOfLine(text, 2))
    }

    @Test
    fun `open bracket adds a continuation level`() {
        val text = "foo(\n  a,\n  b,\n)\n"
        assertEquals(SauleIndent(0, 0), indentOfLine(text, 0))
        assertEquals(SauleIndent(0, 1), indentOfLine(text, 1))
        assertEquals(SauleIndent(0, 1), indentOfLine(text, 2))
        assertEquals(SauleIndent(0, 0), indentOfLine(text, 3))
    }

    @Test
    fun `a blank line takes the enclosing block's indent`() {
        val text = "class A\n\n  fn f()\n\nend\n"
        assertEquals(SauleIndent(1, 0), indentOfLine(text, 1))
        // Inside `fn f()`, still open at this point.
        assertEquals(SauleIndent(2, 0), indentOfLine(text, 3))
    }

    @Test
    fun `a closer typed at the body indent still resolves one level out`() {
        // What the editor sees mid-keystroke: Enter has indented the line to
        // the body level and the closer has just been typed into it. The
        // answer must not depend on the whitespace already there.
        val openers =
            listOf("fn f()", "if a then", "while a do", "for i in x do", "Canvas() do", "try", "match x")
        for (opener in openers) {
            val text = "class A\n  $opener\n    end\n"
            assertEquals(opener, SauleIndent(1, 0), indentOfLine(text, 2))
        }
        assertEquals(SauleIndent(1, 0), indentOfLine("class A\n  repeat\n    until done\n", 2))
        assertEquals(SauleIndent(1, 0), indentOfLine("class A\n  if a then\n    else\n", 2))
        assertEquals(SauleIndent(1, 0), indentOfLine("class A\n  try\n    catch e: any\n", 2))
    }

    @Test
    fun `a closer that turns out to be an identifier keeps the body indent`() {
        // `end` dedents as it is typed, so `endless` has to put it back.
        assertEquals(SauleIndent(2, 0), indentOfLine("class A\n  fn f()\n    endless()\n", 2))
    }

    @Test
    fun `keywordTypedAt fires only on a bare closer`() {
        assertTrue(keywordTypedAt("fn f()\n  end"))
        assertTrue(keywordTypedAt("fn f()\n  else"))
        assertTrue(keywordTypedAt("repeat\n  until"))
        assertTrue(keywordTypedAt("match x\n  case"))
        // One character past a closer: the indent has to be restored.
        assertTrue(keywordTypedAt("fn f()\n  endl"))
        // Half-typed, mid-expression, or not a keyword at all.
        assertFalse(keywordTypedAt("fn f()\n  en"))
        assertFalse(keywordTypedAt("fn f()\n  x = end"))
        assertFalse(keywordTypedAt("fn f()\n  endles"))
        assertFalse(keywordTypedAt("fn f()\n  "))
    }

    @Test
    fun `render uses tabs when the code style asks for them`() {
        val spaces = options(useTabs = false)
        assertEquals("    ", SauleIndent(2, 0).render(spaces))
        assertEquals("      ", SauleIndent(1, 1).render(spaces))

        val tabs = options(useTabs = true, smartTabs = false)
        assertEquals("\t\t", SauleIndent(2, 0).render(tabs))
        assertEquals("\t\t", SauleIndent(1, 1).render(tabs))

        val smart = options(useTabs = true, smartTabs = true)
        assertEquals("\t    ", SauleIndent(1, 1).render(smart))
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    private fun options(useTabs: Boolean, smartTabs: Boolean = true) =
        CommonCodeStyleSettings.IndentOptions().apply {
            INDENT_SIZE = 2
            CONTINUATION_INDENT_SIZE = 4
            TAB_SIZE = 2
            USE_TAB_CHARACTER = useTabs
            SMART_TABS = smartTabs
        }

    /** As the editor asks it: caret at the end of the half-typed text. */
    private fun keywordTypedAt(text: String): Boolean =
        SauleReindent.keywordTypedAt(text, text.length)

    private fun indentOfLine(text: String, line: Int): SauleIndent {
        val starts = lineStarts(text)
        val start = starts[line]
        var end = start
        while (end < text.length && text[end] != '\n') end++
        return SauleIndentModel.indentForLine(text, start, end)
    }

    private fun lineStarts(text: String): List<Int> =
        buildList {
            add(0)
            text.forEachIndexed { i, c -> if (c == '\n' && i + 1 <= text.length) add(i + 1) }
        }

    /**
     * Asserts that re-deriving each line's indent reproduces the sample, i.e.
     * that a file already in canonical form is a fixed point of the model.
     */
    private fun assertRoundTrips(sample: String) {
        val text = sample.trimIndent().trim() + "\n"
        val starts = lineStarts(text)
        text.lines().forEachIndexed { i, line ->
            if (i >= starts.size || line.isBlank()) return@forEachIndexed
            val expected = line.takeWhile { it == ' ' }.length / 2
            val actual = indentOfLine(text, i)
            assertEquals(
                "line ${i + 1}: \"$line\"",
                SauleIndent(expected, 0),
                actual,
            )
        }
    }
}
