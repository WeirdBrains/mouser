class Mouser < Formula
  desc "Mouser terminal emulator"
  homepage "https://github.com/m3talsmith/mouser"
  version "0.7.1"
  on_macos do
    on_arm do
      url "https://github.com/m3talsmith/mouser/releases/download/v0.7.1/mouser-aarch64-apple-darwin.tar.gz"
      sha256 ""
    end
    on_intel do
      url "https://github.com/m3talsmith/mouser/releases/download/v0.7.1/mouser-x86_64-apple-darwin.tar.gz"
      sha256 ""
    end
  end
  def install
    bin.install "mouser-app" => "mouser"
  end
end
