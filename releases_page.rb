class ReleasesPage < BasePage
  ONLY_INTEL_BINARIES = {
    'macOS' => ['x86_64-apple-darwin'],
    'Linux' => ['x86_64-unknown-linux-gnu'],
  }

  INTEL_AND_APPLE_ARM_BINARIES = {
    'macOS (Intel)' => ['x86_64-apple-darwin'],
    'macOS (ARM)' => ['aarch64-apple-darwin'],
    'Linux' => ['x86_64-unknown-linux-gnu'],
  }

  def self.page_title
    'jless - Releases'
  end

  def self.page_path
    '/releases.html'
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

      footer {
        margin-top: 24px;
      }
    CSS
  end

  def self.render_contents
    releases = [
      v0_9_0_release,
      v0_8_0_release,
      v0_7_2_release,
      v0_7_1_release,
      v0_7_0_release,
    ]

    h2(id: 'releases') {"Releases"} + releases.join("\n")
  end

  def self.release_header(version)
    h3(id: version) {version}
  end

  def self.section(header, items)
    h4(header) + ul do
      items.map {|i| li(i)}.join("\n")
    end
  end

  def self.issue(num)
    a(href: "https://github.com/PaulJuliusMartinez/jless/issues/#{num}") do
      "Issue ##{num}"
    end
  end

  def self.pr(num)
    a(href: "https://github.com/PaulJuliusMartinez/jless/pull/#{num}") do
      "PR ##{num}"
    end
  end

  def self.render_binaries(binaries, version)
    lines = []
    binaries.each.with_index do |(platform, architectures), i|
      lines << nil if i != 0
      lines << "# #{platform}"
      architectures.each do |arch|
        lines << "https://github.com/PaulJuliusMartinez/jless/releases/download/#{version}/jless-#{version}-#{arch}.zip"
      end
    end

    h4('Binaries') + code_block(lines, prefix: nil)
  end

  def self.footer_image_src
    './assets/logo/mascot-peanut-butter-jelly-sandwich.svg'
  end

  # Individual Releases

  def self.v0_9_0_release
    release_boilerplate('v0.9.0', INTEL_AND_APPLE_ARM_BINARIES) do
      p1 = p(<<~P)
        The newest version of jless ships with a handful of new helpful features:
      P

      feature1 = li(<<~LI)
        A #{code('ys')} command to copy unescaped string literals to the clipboard
      LI

      feature2 = li(<<~LI)
        A family of printing #{code('p')} commands, analogous to the #{code('y')}
        commands, that print simply content to the screen. Useful for viewing long
        string values, or if the clipboard functionality isn't working
      LI

      feature3 = li(<<~LI)
        Line numbers! Both absolute and relative
      LI

      feature4 = li(<<~LI)
        #{code('C')} and #{code('E')} commands, analogous to the existing #{code('c')} and #{code('e')} commands, for deeply collapsing/expanding values
      LI

      main_features = ul([feature1, feature2, feature3, feature4].join("\n"))

      new_features = section('Full list of new features', [
        <<~ITEM,
          A new command #{code('ys')} will copy unescaped string literals to the
          clipboard. Control characters remain escaped.
        ITEM
        <<~ITEM,
          The length of Arrays and size of Objects is now shown before the container previews
        ITEM
        <<~ITEM,
          Add a new family of "print" commands, that nearly map to the existing copy
          commands, that will simply print a value to the screen. This is useful for
          viewing the entirety of long string values all at once, or if the clipboard
          functionality is not working; mouse-tracking will be temporarily disabled,
          allowing you to use your terminal's native clipboard capabilities to select
          and copy the desired text.
        ITEM
        p(<<~P) +
          Support showing line numbers, both absolute and/or relative. Absolute line
          numbers refer to what line number a given node would appear on if the document
          were pretty printed. This means there are discontinuities when in data mode because
          closing brackets and braces aren't displayed. Relative line numbers show how far a
          line is relative to the currently focused line. The behavior of the various
          combinations of these settings matches vim: when using just relative line numbers
          alone, the focused line will show 0, but when both flags are enabled the focused
          line will show its absolute line number.
        P
        p(<<~P) +
          Absolute line numbers are enabled by default, but not relative line numbers. These
          can be enabled/disabled/re-enabled via command line flags #{code('--line-numbers')},
          #{code('--no-line-numbers')}, #{code('--relative-line-numbers')} and
          #{code('--no-relative-line-numbers')}, or via the short flags #{code('-n')},
          #{code('-N')}, #{code('-r')}, and #{code('-R')} respectively.
        P
        p(<<~P) +
          These settings can also be modified while jless is running. Entering
          #{code(':set number')}/#{code(':set relativenumber')} will enable these settings,
          #{code(':set nonumber')}/#{code(':set norelativenumber')} will disable them, and
          #{code(':set number!')}/#{code(':set relativenumber!')} will toggle them, matching
          vim's behavior.
        P
        p(<<~P),
          There is not yet support for a jless config file, so if you would like relative
          line numbers by default, it is recommended to set up an alias:
          #{code('alias jless=jless --line-numbers --relative-line-numbers')}.
        P
        <<~ITEM,
          You can jump to an exact line number using #{code('<count>g')} or
          #{code('<count>G')}. When using #{code('<count>g')} (lowercase 'g'), if the
          desired line number is hidden inside of a collapsed container, the last visible
          line number before the desired one will be focused. When using #{code('<count>G')}
          (uppercase 'G'), all the ancestors of the desired line will be expanded to
          ensure it is visible.
        ITEM
        <<~ITEM,
          Add #{code('C')} and #{code('E')} commands, analogous to the existing #{code('c')}
          and #{code('e')} commands, to deeply collapse/expand a node and all its siblings.
        ITEM
      ])

      improvements = section('Improvements', [
        <<~ITEM,
          In data mode, when a array element is focused, the highlighting on the index
          label (e.g., #{code('[8]')}) is now inverted. Additionally, a #{code('▶')} is always
          displayed next to the currently focused line, even if the focused node is a
          primitive. Together these changes should make it more clear which line is
          focused, especially when the terminal's current style doesn't support
          dimming (#{code('ESC [ 2 m')}).
        ITEM
        <<~ITEM,
          When using the #{code('c')} and #{code('e')} commands (and the new #{code('C')}
          and #{code('E')} commands), the focused row will stay at the same spot on the
          screen. (Previously jless would try to keep the same row visible at the top of
          the screen, which didn't make sense.)
        ITEM
      ])

      bug_fixes = section('Bug Fixes', [
        <<~ITEM,
          Scrolling with the mouse will now move the viewing window, rather than the cursor.
        ITEM
        <<~ITEM,
          When searching, jless will do a better job jumping to the first match after the
          cursor; previously if a user started a search while focused on the opening of a
          Object or Array, any matches inside that container were initially skipped over.
        ITEM
        <<~ITEM,
          When jumping to a search match that is inside a collapsed container, search
          matches will continue to be highlighted after expanding the container.
        ITEM
        <<~ITEM,
          [#{issue(71)}/#{pr(98)}]: jless will return a non-zero exit code if it fails to
          parse the input.
        ITEM
      ])

      other_notes = section('Other Notes', [
        <<~ITEM,
          The minimum supported Rust version has been updated to 1.67.
        ITEM
        <<~ITEM,
          jless now re-renders the screen by emitting "clear line" escape codes
          (#{code('ESC [ 2 K')}) for each line, instead of a single "clear screen" escape
          #{code('code (ESC [ 2 J')}), in the hopes of reducing flicking when scrolling.
        ITEM
      ])

      [p1, main_features, new_features, improvements, bug_fixes, other_notes].join("\n")
    end
  end

  def self.v0_8_0_release
    release_boilerplate('v0.8.0', ONLY_INTEL_BINARIES) do
      p1 = p(<<~P)
        This release ships with two major new features: basic YAML support
        and copying to clipboard!
      P

      p2 = p(<<~P)
        #{code('jless')} will now check the file extension of the input file,
        and automatically parse #{code('.yml')} and #{code('.yaml')} files as
        YAML and use the same viewer as for JSON data. Alternatively passing in
        the #{code('--yaml')} flag will force #{code('jless')} to parse the
        input as YAML and can be used when reading in YAML data from stdin.
        YAML aliases are automatically expanded, but their corresponding anchors
        are not visible, nor are comments. YAML supports non-string keys, and
        even non-scalar keys in mappings (e.g., the key of map can be an array
        with multiple elements). Non-string keys are shown with square brackets,
        e.g., #{code('[true]: "value"')}, instead of quotes. Non-scalar keys are
        handled on the screen and displayed properly, but you cannot expand and
        collapse their individual elements.
      P

      p3 = p(<<~P)
        While navigating data, jless also now supports copying various items to
        your system clipboard.
      P

      copy_commands = ul do
        yy = li(<<~LI)
          #{code('yy')} will copy the value of the currently focused node,
          pretty printed
        LI
        yv = li(<<~LI)
          #{code('yv')} will copy the value of the currently focused node in a
          "nicely" printed one-line format
        LI
        yk = li("#{code('yk')} will copy the key of the current key/value pair")
        yp = li(<<~LI)
          #{code('yp')} will copy the path from the root JSON element to the
          currently focused node, e.g., #{code('.foo[3].bar')}
        LI
        yb = li(<<~LI)
          #{code('yb')} functions like #{code('yp')}, but always uses the
          bracket form for object keys, e.g., #{code('["foo"][3]["bar"]')},
          which is useful if the environment where you'll paste the path doesn't
          support the #{code('.key')} format, like in Python
        LI

        jq_link = code {a(href: 'https://stedolan.github.io/jq/') {'jq'}}
        yq = li(<<~LI)
          #{code('yq')} will copy a #{jq_link} style path that will select the
          currently focused node, e.g., #{code('.foo[].bar')}
        LI

        [yy, yv, yk, yp, yb, yq].join("\n")
      end

      new_features = section('Other new features', [
        <<~ITEM,
          Implement #{code('ctrl-u')} and #{code('ctrl-d')} commands to jump up
          and down by half the screen's height, or by a specified number of lines.
        ITEM
        <<~ITEM,
          Implement #{code('ctrl-b')} and #{code('ctrl-f')} commands for
          scrolling up and down by the height of the screen. (Aliases for
          #{code('PageUp')} and #{code('PageDown')})
        ITEM
      ])

      improvements = section('Improvements', [
        <<~ITEM,
          Keep focused line in same place on screen when toggling between line
          and data modes; fix a crash when focused on a closing delimiter and
          switching to data mode.
        ITEM
        <<~ITEM,
          Pressing Escape will clear the input buffer and stop highlighting
          search matches.
        ITEM
      ])

      bug_fixes = section('Bug fixes', [
        <<~ITEM,
          Ignore clicks on the status bar or below rather than focusing on
          hidden lines, and don't re-render the screen, allowing the path in
          the status bar to be highlighted and copied.
        ITEM
        <<~ITEM,
          [#{issue(61)}]: Display error message for unrecognized CSI escape
          sequences and other IO errors instead of panicking.
        ITEM
        <<~ITEM,
          [#{issue(62)}]: Fix broken window resizing / SIGWINCH detection
          caused by clashing signal handler registered by rustyline.
        ITEM
        <<~ITEM,
          [#{pr(54)}]: Fix panic when using #{code('ctrl-c')} or
          #{code('ctrl-d')} to cancel entering search input.
        ITEM
      ])

      cve_2022_24713 = a(href: "https://blog.rust-lang.org/2022/03/08/cve-2022-24713.html") do
        'CVE-2022-24713'
      end
      other_notes = section('Other notes', [
        <<~ITEM,
          Upgraded regex crate to 1.5.5 due to #{cve_2022_24713}.
        ITEM
      ])

      [p1, p2, p3, copy_commands, new_features, improvements, bug_fixes, other_notes].join("\n")
    end
  end

  def self.v0_7_2_release
    release_boilerplate('v0.7.2', ONLY_INTEL_BINARIES) do
      new_features_and_changes = section('New features', [
        <<~ITEM,
          Space now toggles the collapsed state of the currently focused
          node, rather than moving down a line. (Functionality was
          previously available via #{code('i')}, but was undocumented;
          #{code('i')} has become unmapped.)
        ITEM
      ])

      bug_fixes = section('Bug fixes', [
        <<~BUG,
          [#{issue(7)} / #{pr(32)}]: Searching now works even when input is
          provided via #{code('stdin')}
        BUG
      ])

      internal = section('Internal code cleanup', [
        "[#{pr(17)}]: Upgrade from structopt to clap v3",
      ])

      [new_features_and_changes, bug_fixes, internal].join("\n")
    end
  end

  def self.v0_7_1_release
    release_boilerplate('v0.7.1', ONLY_INTEL_BINARIES) do
      new_features = section('New features', [
        'F1 now opens help page',
        <<~ITEM,
          Search initialization commands (#{code('/')}, #{code('?')},
          #{code('*')}, #{code('#')}) all now accept count arguments
        ITEM
      ])

      code_cleanup = section('Internal code cleanup', [
        'Address a lot of issues reported by clippy',
        'Remove chunks of unused code, including serde dependency',
        'Fix typos in help page',
      ])

      [new_features, code_cleanup].join("\n")
    end
  end

  def self.v0_7_0_release
    this_github_issue =
      a(href: 'https://github.com/PaulJuliusMartinez/jless/issues/1') {"This GitHub issue"}

    content = p('Introducing jless, a command-line JSON viewer.') +
      p(<<~P) +
        This release represents a significant milestone: a complete set of
        basic functionality, without any major bugs.
      P
      p(<<~P) +
        #{this_github_issue} details much of the functionality implemented to
        get to this point. Spiritually, completion of many of the tasks listed
        there represent versions 0.1 - 0.6.
      P
      p('The intention is to not release a 1.0 version until Windows support is added.')

    release_boilerplate('v0.7.0', ONLY_INTEL_BINARIES) {content}
  end

  def self.release_boilerplate(version, binaries, &block)
    div(klass: 'release') do
      release_header(version) + block.call + render_binaries(binaries, version)
    end
  end
end
