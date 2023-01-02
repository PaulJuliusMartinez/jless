class HomePage < BasePage
  GIF_PATH = './assets/jless-recording.gif'.freeze
  GITHUB_RELEASES = 'https://github.com/PaulJuliusMartinez/jless/releases'.freeze

  PACKAGE_MANAGERS = [
    { os: 'macOS', tool: 'HomeBrew', command: 'brew install jless', link: 'https://formulae.brew.sh/formula/jless' },
    { os: 'macOS', tool: 'MacPorts', command: 'sudo port install jless', link: 'https://ports.macports.org/port/jless/' },
    { os: 'Linux', tool: 'HomeBrew', command: 'brew install jless', link: 'https://formulae.brew.sh/formula/jless' },
    { os: 'Arch Linux', command: 'pacman -S jless', link: 'https://archlinux.org/packages/community/x86_64/jless/' },
    { os: 'Void Linux', command: 'sudo xbps-install jless', link: 'https://github.com/void-linux/void-packages/tree/master/srcpkgs/jless' },
    { os: 'NetBSD', command: 'pkgin install jless', link: 'https://pkgsrc.se/textproc/jless/' },
    { os: 'FreeBSD', command: 'pkg install jless', link: 'https://freshports.org/textproc/jless/' },
  ].freeze

  def self.page_css
    <<~CSS
      #jless-recording {
        margin: 0 auto;
        max-width: min(540px, 100%);
      }

      #jless-recording {
        display: block;
        margin-bottom: 0.5em;
      }

      .text-and-mascot {
        display: flex;
        justify-content: space-between;
        align-items: center;
      }

      .text-and-mascot img {
        width: 30%;
        padding: 16px;
      }

      #installation-table {
        border-collapse: collapse;
        width: 100%;
      }

      #installation-table tbody td {
        border: 1px solid black;
        border-radius: 4px;
        padding: 4px 8px;
      }

      #installation-table code {
        font-size: 16px;
      }

      @media (max-width: 540px) {
        .text-and-mascot {
          flex-wrap: wrap;
          justify-content: center;
        }

        .text-and-mascot img {
          order: 5;
          width: 180px;
          padding: 0 16px;
        }
      }
    CSS
  end

  def self.render_contents
    intro = p(<<~P)
      jless is a command-line JSON viewer designed for reading, exploring,
      and searching through JSON data.
    P
    gif = img(id: 'jless-recording', src: GIF_PATH)
    features = render_features
    installation = render_installation
    user_guide = p(<<~P)
      Check out the #{a(href: './user-guide.html') {'user guide'}} to learn
      about the full functionality of jless.
    P

    intro + gif + features + installation + user_guide
  end

  def self.render_features
    features = [
      {
        copy: <<~COPY,
          jless will pretty print your JSON and apply syntax highlighting.
          Use it when exploring external APIs, or debugging request payloads.
        COPY
        img: './assets/logo/mascot-indentation.svg',
      },
      {
        copy: <<~COPY,
          Expand and collapse Objects and Arrays to grasp the high- and low-level
          structure of a JSON document. jless has a large suite of vim-inspired
          commands that make exploring data a breeze.
        COPY
        img: './assets/logo/mascot-rocks-collapsing.svg',
      },
      {
        copy: <<~COPY,
          jless supports full text regular-expression based search. Quickly find
          the data you're looking for in long String values, or jump between
          values for the same Object key.
        COPY
        img: './assets/logo/mascot-searching.svg',
      },
    ]

    features.map.with_index do |feature, i|
      div(klass: 'text-and-mascot') do
        copy = p {feature[:copy]}
        picture = img(src: feature[:img])

        if i.even?
          copy + picture
        else
          picture + copy
        end
      end
    end.join
  end

  def self.render_installation
    intro = h2 {"Installation"} + p(<<~P)
      jless currently supports macOS and Linux and can be installed using
      various package managers.
    P

    package_managers = table(id: 'installation-table') do
      header_row = tr do
        th {'OS'} +
          th {'Package Manager'} +
          th {'Command'}
      end

      rows = PACKAGE_MANAGERS.map do |pkg|
        cells = []

        if pkg[:tool]
          cells << td {pkg[:os]}
          cells << td do
            a(href: pkg[:link]) {pkg[:tool]}
          end
        else
          cells << td(colspan: '2') do
            a(href: pkg[:link]) {pkg[:os]}
          end
        end

        cells << td {code {pkg[:command]}}

        tr {cells.join}
      end.join

      thead {header_row} + tbody {rows}
    end

    cargo = p do
      msg = 'If you have a Rust toolchain installed, you can also install directly from source using cargo:'
      msg + code_block(['cargo install jless'])
    end

    github_releases = a(href: GITHUB_RELEASES) {'GitHub'}
    binaries = p(<<~P)
      Binaries for various architectures are also available on #{github_releases}.
    P

    intro + package_managers + cargo + binaries
  end
end
