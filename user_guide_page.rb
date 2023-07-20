class UserGuidePage < BasePage
  N = '<code>N</code>'
  REGEX_LINK = 'https://docs.rs/regex/latest/regex/index.html#syntax'

  def self.page_title
    'jless - User Guide'
  end

  def self.page_path
    '/user-guide.html'
  end

  def self.page_css
    <<~CSS
      code {
        position: relative;
        top: -2px;
        display: inline-block;
        border: 1px solid black;
        border-radius: 4px;
        margin: 0 2px;
        padding: 2px 4px;
        background-color: #eeeeee;
        font-size: 16px;
      }
    CSS
  end

  def self.render_contents
    render_basic_usage + render_commands
  end

  def self.render_basic_usage
    jq_link = a(href: "https://stedolan.github.io/jq/") {'jq'}

    h2(id: 'usage') {'Usage'} +
      p('jless can read files directly, or read JSON data from standard input:') +
      code_block([
        'curl https://api.github.com/repos/PaulJuliusMartinez/jless/commits -o commits.json',
        'jless commits.json',
        'cat commits.json | jless',
      ]) +
      p(<<~P) +
        jless can handle newline-delimited JSON, so feel free to pipe in the
        output from #{jq_link} or some dense log files.
      P
      p(<<~P) +
        jless can also handle YAML data, either automatically by detecting
        the file extension, or by explicitly passing the #{code('--yaml')} flag.
        If you frequently view YAML data, we suggest the following alias:
      P
      code_block(['alias yless="jless --yaml"'])
  end

  def self.render_commands
    h2(id: 'commands') {'Commands'} +
      p(<<~P) +
        jless has a large suite of vim-inspired commands. Commands prefixed by
        <i>count</i> may be preceded by a number #{N}, which will
        perform a command a given number of times.
      P
      render_util_commands +
      render_movement_commands +
      render_scrolling_commands +
      render_copying_and_printing_commands +
      render_search_commands +
      render_search_input_explanation +
      render_modes +
      render_line_numbers
  end

  def self.render_util_commands
    ul do
      command(%w[q Ctrl-C :quit :exit], "Exit jless; don't worry, it's not as hard as exiting vim.") +
        command(%w[:help F1], 'Show the help page.')
    end
  end

  def self.render_movement_commands
    h3(id: 'moving') {'Moving'} + ul do
      count_command(%w[j DownArrow Ctrl-n Enter], "Move focus down one line (or #{N} lines).") +
      count_command(%w[k UpArrow Ctrl-p Backspace], "Move focus up one line (or #{N} lines).") +
      command(%w[h LeftArrow], <<~DESC) +
        When focused on an expanded object or array, collapse the object or
        array. Otherwise, move focus to the parent of the focused node.
      DESC
      command(%w[H], 'Focus the parent of the focused node without collapsing the focused node.') +
      command(%w[l RightArrow], <<~DESC) +
        When focused on a collapsed object or array, expand the object or array.
        When focused on an expanded object or array, move focus to the first
        child. When focused on non-container values, does nothing.
      DESC
      count_command(%w[J], "Move to the focused node's next sibling 1 or #{N} times.") +
      count_command(%w[K], "Move to the focused node's previous sibling 1 or #{N} times.") +
      count_command(%w[w], "Move forward until the next change in depth 1 or #{N} times.") +
      count_command(%w[b], "Move backwards until the next change in depth 1 or #{N} times.") +
      count_command(%w[Ctrl-f PageDown], "Move down by one window's height or #{N} windows' heights.") +
      count_command(%w[Ctrl-b PageUp], "Move up by one window's height or #{N} windows' heights.") +
      command(%w[0 ^], "Move to the first sibling of the focused node's parent.") +
      command(%w[$], "Move to the last sibling of the focused node's parent.") +
      command(%w[Home], 'Focus the first line in the input') +
      command(%w[End], 'Focus the last line in the input') +
      count_command(%w[g], <<~DESC) +
        Focus the first line in the input if no count is given. If a count is given, focus
        that line number. If the line isn't visible, focus the last visible line before it.
      DESC
      count_command(%w[G], <<~DESC) +
        Focus the last line in the input if no count is given. If a count is given, focus
        that line number, expanding any of its parent nodes if necessary.
      DESC
      command(%w[c], 'Shallow collapse the focused node and all its siblings.') +
      command(%w[C], 'Deep collapse the focused node and all its siblings.') +
      command(%w[e], 'Shallow expand the focused node and all its siblings.') +
      command(%w[E], 'Deep expand the focused node and all its siblings.') +
      command(%w[Space], 'Toggle the collapsed state of the currently focused node.')
    end
  end

  def self.render_scrolling_commands
    h3(id: 'scrolling') {"Scrolling"} + ul do
      count_command(%w[Ctrl-e], "Scroll down one line (or #{N} lines).") +
        count_command(%w[Ctrl-y], "Scroll up one line (or #{N} lines).") +
        count_command(%w[Ctrl-d], "Scroll down by half the height of the screen (or by #{N} lines).") +
        count_command(%w[Ctrl-u], <<~CMD) +
          Scroll up by half the height of the screen (or by #{N} lines). For
          this command and <code>Ctrl-d</code>, focus is also moved by the
          specified number of lines. If no count is specified, the number of
          lines to scroll by is recalled from previous executions.
        CMD
        command(%w[zz], "Move the focused node to the center of the screen.") +
        command(%w[zt], "Move the focused node to the top of the screen.") +
        command(%w[zb], "Move the focused node to the bottom of the screen.") +
        count_command(%w[.], "Scroll a truncated value one character to the right (or #{N} characters).") +
        count_command(%w[,], "Scroll a truncated value one character to the left (or #{N} characters).") +
        command(%w[;], <<~CMD) +
          Scroll a truncated value all the way to the end, or, if already at
          the end, back to the start.
        CMD
        count_command(%w[&lt;], "Decrease the indentation of every line by one (or #{N}) tabs.") +
        count_command(%w[&gt;], "Increase the indentation of every line by one (or #{N}) tabs.")
    end
  end

  def self.render_copying_and_printing_commands
    copy_commands = ul do
      yy_pp = command(['yy', 'pp'], 'Copy/print the value of the currently focused node, pretty printed')
      yv_pv = command(['yv', 'pv'], 'Copy/print the value of the currently focused node in a "nicely" printed one-line format')
      ys_ps = command(['ys', 'ps'], <<~DESC)
        When the currently focused value is a string, copy/print the contents of the
        string unescaped (except control characters)
      DESC
      yk_pk = command(['yk', 'pk'], 'Copy/print the key of the current key/value pair')
      yp_pP = command(['yp', 'pP'], <<~CMD)
        Copy/print the path from the root JSON element to the currently focused
        node, e.g., #{code('.foo[3].bar')}
      CMD
      yb_pb = command(['yb', 'pb'], <<~CMD)
        Like #{code('yp')}, but always uses the bracket form for object keys,
        e.g., #{code('["foo"][3]["bar"]')}, which is useful if the environment
        where you'll paste the path doesn't support the #{code('.key')} format,
        like in Python
      CMD

      jq_link = code {a(href: 'https://stedolan.github.io/jq/') {'jq'}}
      yq_pq = command(['yq', 'pq'], <<~CMD)
        Copy/print a #{jq_link} style path that will select the currently focused
        node, e.g., #{code('.foo[].bar')}
      CMD

      [yy_pp, yv_pv, ys_ps, yk_pk, yp_pP, yb_pb, yq_pq].join("\n")
    end

    h3(id: 'copying-and-printing') {'Copying and Printing'} +
      p(<<~P) +
        You can copy various parts of the JSON file to your clipboard using
        one of the following #{code('y')} commands.
      P
      p(<<~P) +
        Alternatively, you can print out values using #{code('p')}. This is useful
        for viewing long string values all at once, or if the clipboard functionality
        is not working; mouse-tracking will be temporarily disabled, allowing you
        to use your terminal's native clipboard capabilities to select and copy
        the desired text.
      P
      copy_commands
  end

  def self.render_search_commands
    search_commands = ul do
      count_command(['/pattern'], <<~CMD) +
        Search forward for the given pattern, or to its #{N}th occurrence.
      CMD
        count_command(['?pattern'], <<~CMD) +
          Search backwards for the given pattern, or to its #{N}th previous occurrence.
        CMD
        count_command(['*'], <<~CMD) +
          Move to the next occurrence of the object key on the focused line
          (or move forward #{N} occurrences).
        CMD
        count_command(['#'], <<~CMD) +
          Move to the previous occurrence of the object key on the focused line
          (or move backwards #{N} occurrences).
        CMD
        count_command(['n'], <<~CMD) +
          Move in the search direction to the next match (or forward #{N} matches).
        CMD
        count_command(['N'], <<~CMD)
          Move in the opposite of the search direction to the previous match
          (or previous #{N} matches).
        CMD
    end

    h3(id: 'search') {'Search'} +
      p('jless supports full-text search over the input JSON.') +
      search_commands +
      p(<<~P) +
        Searching uses "smart case" by default. If the input pattern doesn't
        contain any capital letters, a case insensitive search will be
        performed. If there are any capital letters, it will be case
        sensitive. You can force a case-sensitive search by appending
        <code>/s</code> to your query.
      P
      p(<<~P) +
        A trailing slash will be removed from a pattern; to search for a
        pattern ending in <code>/</code> (or <code>/s</code>), just add
        another <code>/</code> to the end.
      P
      p(<<~P) +
        Search patterns are interpreted as mostly standard regular
        expressions, with one exception. Because JSON data contains many
        square and curly brackets (<code>[]{}</code>), these characters do
        <i>not</i> take on their usual meanings (specifying characters
        classes and repetition counts respectively) and are instead
        interpreted literally.
      P
      p(<<~P) +
        To use character classes or repetition counts, escape these
        characters with a backslash.
      P
      p('Some examples:') +
      ul do
        li {'<code>/[1, 2, 3]</code> matches an array: <code>[1, 2, 3]</code>'} +
          li {'<code>/\[bch\]at</code> matches <code>bat</code>, <code>cat</code> or <code>hat</code>'} +
          li {'<code>/{}</code> matches an empty object <code>{}</code>'} +
          li {'<code>/(ha)\{2,3\}</code> matches <code>haha</code> or <code>hahaha</code>'}
      end +
      p(<<~P)
        For exhaustive documentation of the supported regular expression
        syntax, check out the
        #{a(href: REGEX_LINK) {'documentation of the underlying regex engine:'}}.
      P
  end

  def self.render_search_input_explanation
    h3(id: 'search-input') {'Search Input'} +
      p(<<~P) +
        The search is <i>not</i> performed over the original input, but over a
        single-line pretty formatted version of the input JSON. Consider the
        following two ways to format an equivalent JSON blob:
      P
      code_block([
        '{"a":1,"b":true,"c":[null,{},[],"hello"]}',
        nil,
        <<~PRETTY,
          {
            "a": 1,
            "b": true,
            "c": [
              null,
              {},
              [],
            "hello"
            ]
          }
        PRETTY
      ], prefix: nil) +
      p(<<~P) +
        jless will create an internal representation formatted as follows:
      P
      code_block(['{ "a": 1, "b": true, "c": [null, {}, [], "hello"] }'], prefix: nil) +
      p(<<~P) +
        (No spaces inside empty objects or arrays, one space inside objects
        with values, no spaces inside array square brackets, no space between
        an object key and ':', one space after the ':', and one space after
        commas separating object entries and array elements.)
      P
      p(<<~P) +
        Searching will be performed over this internal representation so that
        patterns can include multiple elements without worrying about newlines
        or the exact input format.
      P
      p(<<~P)
        When the input is newline-delimited JSON, an actual newline will
        separate each top-level JSON element in the internal representation.
      P
  end

  def self.render_modes
    h3(id: 'data-mode-vs-line-mode') {"Data Mode vs. Line Mode"} +
      p(<<~P) +
        jless starts in "data" mode, which displays the JSON data in a more
        streamlined fashion: no closing delimiters for objects or arrays, no
        trailing commas, no quotes around object keys that are valid
        identifiers in JavaScript. It also shows single-line previews of
        objects and arrays, and array indexes before array elements. Note that
        when using full-text search, object keys will still be surrounded by
        quotes.
      P
      p(<<~P) +
        By pressing <code>m</code>, you can switch jless to "line" mode, which
        displays the input as pretty printed JSON.
      P
      p(<<~P)
        In line mode you can press <code>%</code> when focused on an open or
        close delimiter of an object or array to jump to its matching pair.
      P
  end

  def self.render_line_numbers
    flag1 = li(<<~LI)
      #{code('-n')}, #{code('--line-numbers')} Show absolute line numbers.
    LI

    flag2 = li(<<~LI)
      #{code('-N')}, #{code('--no-line-numbers')} Don't show absolute line numbers.
    LI

    flag3 = li(<<~LI)
      #{code('-r')}, #{code('--relative-line-numbers')} Show relative line numbers.
    LI

    flag4 = li(<<~LI)
      #{code('-R')}, #{code('--no-relative-line-numbers')} Don't show relative line numbers.
    LI

    cli_flags = ul(flag1 + flag2 + flag3 + flag4)

    setting1 = li(<<~LI)
      #{code(':set number')} Show absolute line numbers.
    LI

    setting2 = li(<<~LI)
      #{code(':set nonumber')} Don't show absolute line numbers.
    LI

    setting3 = li(<<~LI)
      #{code(':set number!')} Toggle whether showing absolute line numbers.
    LI

    setting4 = li(<<~LI)
      #{code(':set relativenumber')} Show relative line numbers.
    LI

    setting5 = li(<<~LI)
      #{code(':set norelativenumber')} Don't show relative line numbers.
    LI

    setting6 = li(<<~LI)
      #{code(':set relativenumber!')} Toggle whether showing relative line numbers.
    LI

    settings = [
      setting1,
      setting2,
      setting3,
      setting4,
      setting5,
      setting6,
    ].join("\n")

    h3(id: 'line-numbers') {"Line Numbers"} +
      p(<<~P) +
        jless supports displaying line numbers, and does so by default. The line
        numbers do not reflect the position of a node in the original input, but
        rather what line the node would appear on if the original input were
        pretty printed.
      P
      p(<<~P) +
        jless also supports relative line numbers. When this is enabled, the
        number displayed next to each line will indicate how many lines away it
        is from the currently focused line. This makes it easier to use the
        #{code('j')} and #{code('k')} commands with specified counts.
      P
      p(<<~P) +
        The appearance of line numbers can be configured via command line flags:
      P
      cli_flags +
      p("As well as at runtime:") +
      settings +
      p(<<~P)
        When just using relative line numbers, "0" will be displayed next to the
        currently focused line. When both flags are set, the absolute line
        number will be displayed next to the focused lines, and all other line
        numbers will be relative. This matches vim's behavior.
      P
  end

  def self.command(inputs, description, count: false)
    cmd = ""
    cmd << html_elem('i') {'count'} + ' ' if count
    cmd << inputs.map {|input| code(input)}.join(', ') + ' '
    li {cmd + description}
  end

  def self.count_command(inputs, description)
    command(inputs, description, count: true)
  end

  def self.footer_image_src
    './assets/logo/mascot-rocket.svg'
  end
end
