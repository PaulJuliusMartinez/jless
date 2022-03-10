class ReleasesPage < BasePage
  BINARIES = {
    'macOS' => ['x86_64-apple-darwin'],
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

  def self.v0_8_0_release
    release_boilerplate('v0.8.0', BINARIES) do
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
    release_boilerplate('v0.7.2', BINARIES) do
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
    release_boilerplate('v0.7.1', BINARIES) do
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

    release_boilerplate('v0.7.0', BINARIES) {content}
  end

  def self.release_boilerplate(version, binaries, &block)
    div(klass: 'release') do
      release_header(version) + block.call + render_binaries(binaries, version)
    end
  end
end
