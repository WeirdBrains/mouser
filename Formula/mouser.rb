class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/WeirdBrains/mouser"
  version "1.4.5"
  on_macos do
    on_arm do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.4.5/mouser-aarch64-apple-darwin.tar.gz"
      sha256 ""
    end
    on_intel do
      url "https://github.com/WeirdBrains/mouser/releases/download/v1.4.5/mouser-x86_64-apple-darwin.tar.gz"
      sha256 ""
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
    (lib/"mouser").install "engines"
  end
end
