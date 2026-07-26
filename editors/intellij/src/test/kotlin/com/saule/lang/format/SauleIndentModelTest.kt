package com.saule.lang.format

import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import org.junit.Assert.assertEquals
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
