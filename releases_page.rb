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
