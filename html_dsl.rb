require 'cgi'

# A simple HTML DSL
module HtmlDSL
  def html_elem(elem, content=nil, **attrs, &block)
    attr_str = attrs.entries
      .select {|_, v| v}
      .map do |key, val|
        key = :class if key == :klass
        if val.is_a?(TrueClass) || val.is_a?(FalseClass)
          " #{key}"
        else
          " #{key}=\"#{CGI.escape_html(val)}\""
        end
      end
      .join

    if content || block
      "<#{elem}#{attr_str}>#{content || block.call}</#{elem}>"
    else
      "<#{elem}#{attr_str} />"
    end
  end

  %i[h1 h2 h3 h4 div span p a img ul li code table thead tbody th td tr].each do |elem|
    module_eval(<<~ELEM_METHOD, __FILE__, __LINE__ + 1)
      def #{elem}(content=nil, **attrs, &block)
        html_elem('#{elem}', content, **attrs, &block)
      end
    ELEM_METHOD
  end
end
