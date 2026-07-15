# Homebrew cask for hedos — installs the app and links its bundled CLI.
# Lives in a tap (e.g. theiskaa/homebrew-tap) as Casks/hedos.rb, then:
#   brew install --cask theiskaa/homebrew-tap/hedos
# Set `sha256` to the DMG's checksum (shasum -a 256 dist/Hedos.dmg) before publishing.
cask "hedos" do
  version "0.1.2"
  sha256 "abefe0bf594d2d132cef42cb02ddbb44ed54b33d1b05a166dda14b73866e96d3"

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
