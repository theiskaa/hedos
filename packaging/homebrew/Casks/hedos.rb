# Homebrew cask for hedos — installs the app and links its bundled CLI.
# Lives in a tap (e.g. theiskaa/homebrew-tap) as Casks/hedos.rb, then:
#   brew install --cask theiskaa/homebrew-tap/hedos
# Set `sha256` to the DMG's checksum (shasum -a 256 dist/Hedos.dmg) before publishing.
cask "hedos" do
  version "0.1.1"
  sha256 "e77af3fa3fd07208f122960cb60a4f7b00793a73b3ec78e155ee2048e1765c82"

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
