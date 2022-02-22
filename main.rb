#! /usr/bin/env ruby

JLESS_VERSION = 'v0.7.2'

require './html_dsl'
require './base_page'
require './home_page'
require './user_guide_page'
require './releases_page'

def main
  HomePage.generate('dist/index.html')
  UserGuidePage.generate('dist/user-guide.html')
  ReleasesPage.generate('dist/releases.html')
end

main if __FILE__ == $PROGRAM_NAME
