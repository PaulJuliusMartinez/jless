class BasePage
  extend HtmlDSL

  TWITTER_LINK = 'https://twitter.com/CodeIsTheEnd'.freeze

  def self.generate(filename)
    contents = "<!DOCTYPE html>\n"
    contents << html_elem('html') do
      render_head +
        html_elem('body') do
          render_header + render_contents + render_footer
        end
    end

    File.write(filename, contents)
  end

  def self.page_title
    'jless - A Command-Line JSON Viewer'
  end

  def self.page_path
    ''
  end

  def self.base_css
    @base_css ||= File.read('base.css')
  end

  def self.page_css
    ''
  end

  def self.render_head
    head_tags = <<~HTML
      <meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1.0">
      <title>#{page_title}</title>
      <meta name="twitter:card" content="summary_large_image" />
      <meta name="twitter:creator" content="@CodeIsTheEnd" />
      <meta property="og:type" content="website" />
      <meta property="og:url" content="https://jless.io#{page_path}" />
      <meta property="og:title" content="#{page_title}" />
      <meta property="og:description" content="jless is a command-line JSON viewer designed for reading, exploring, and searching through JSON data." />
      <meta property="og:image" content="https://jless.io/assets/logo/text-logo-with-mascot-social.png" />
      <link rel="icon" href="./assets/logo/mascot.svg" type="image/svg+xml">
      <link rel="preconnect" href="https://fonts.googleapis.com">
      <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
      <link href="https://fonts.googleapis.com/css2?family=Fira+Sans:wght@400;700&family=Roboto+Slab:wght@800&display=swap" rel="stylesheet">
    HTML

    html_elem('head') do
      head_tags +
        html_elem('style') { base_css + page_css }
    end
  end

  def self.footer_image_src
    './assets/logo/mascot.svg'
  end

  def self.render_header
    <<~HTML
      <header>
        <img id="text-logo-with-mascot" src="./assets/logo/text-logo-with-mascot.svg">
        <h2>jless — a command-line JSON viewer</h3>
      </header>
      <nav>
        <a href="./">About</a>
        <a href="./user-guide.html">User Guide</a>
        <a href="./releases.html">Releases</a>
        <a href="https://github.com/PaulJuliusMartinez/jless">GitHub</a>
      </nav>
    HTML
  end

  def self.render_contents
    # Implement me!
  end

  def self.render_footer
    twitter_handle = a(href: TWITTER_LINK) {'CodeIsTheEnd'}

    html_elem('footer') do
      img(src: footer_image_src) +
        div(style: 'margin-top: 24px') do
          "Created by #{twitter_handle}"
        end
    end
  end

  def self.code_block(lines, prefix: '$ ')
    div(klass: 'code-block') do
      lines.map do |line|
        next '' if !line

        if prefix
          span(klass: 'prefix') {'$ '} + line
        else
          line
        end
      end.join("\n")
    end
  end
end
