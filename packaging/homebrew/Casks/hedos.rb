# Homebrew cask for hedos — installs the app and links its bundled CLI.
# Lives in a tap (e.g. theiskaa/homebrew-tap) as Casks/hedos.rb, then:
#   brew install --cask theiskaa/homebrew-tap/hedos
# Set `sha256` to the DMG's checksum (shasum -a 256 dist/Hedos.dmg) before publishing.
cask "hedos" do
  version "0.1.4"
  sha256 "239f536add825364aee069aac0404bf2895f5e2256d3c1d26a232ab3ce160d3b"

  url "https://github.com/theiskaa/hedos/releases/download/v#{version}/Hedos.dmg"
  name "Hedos"
  desc "One native home for every local model on your Mac"
  homepage "https://github.com/theiskaa/hedos"

  depends_on macos: :tahoe

  app "Hedos.app"
  binary "#{appdir}/Hedos.app/Contents/Helpers/hedos"

  zap trash: [
    "~/Library/Application Support/Hedos",
    "~/Library/Preferences/dev.theiskaa.hedos.plist",
  ]
end
