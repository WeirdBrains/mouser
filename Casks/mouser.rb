cask "mouser" do
  version "1.6.0"
  on_arm do
    sha256 "b284b6ee9cc768c830c227d7849a89676207cdf311f7ef68cc68b4942b9b34e4"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.6.0/Mouser-aarch64-apple-darwin.dmg"
  end
  on_intel do
    sha256 "86edaeeb97152fed16f0f871288ad89436876319440b99046c6db1c8d5024c1b"
    url "https://github.com/WeirdBrains/mouser/releases/download/v1.6.0/Mouser-x86_64-apple-darwin.dmg"
  end
  name "Mouser"
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  auto_updates false
  app "Mouser.app"
end
