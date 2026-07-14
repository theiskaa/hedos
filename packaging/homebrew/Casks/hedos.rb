# Homebrew cask for hedos — installs the app and links its bundled CLI.
# Lives in a tap (e.g. theiskaa/homebrew-tap) as Casks/hedos.rb, then:
#   brew install --cask theiskaa/homebrew-tap/hedos
# Set `sha256` to the DMG's checksum (shasum -a 256 dist/Hedos.dmg) before publishing.
cask "hedos" do
  version "0.1.0"
  sha256 "88b426a939ec40b29613ed6c3f0fb96641892e10461162426c0bddd0deec2e3b"

  url "https://github.com/theiskaa/hedos/releases/download/v#{version}/Hedos.dmg"
  name "Hedos"
  desc "One native home for every local model on your Mac"
  homepage "https://github.com/theiskaa/hedos"

  depends_on macos: ">= :sequoia"

  app "Hedos.app"
  binary "#{appdir}/Hedos.app/Contents/Helpers/hedos"

  zap trash: [
    "~/Library/Application Support/Hedos",
    "~/Library/Preferences/dev.theiskaa.hedos.plist",
  ]
end
