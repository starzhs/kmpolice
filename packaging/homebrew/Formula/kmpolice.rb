class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/<ORG_OR_USER>/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/<ORG_OR_USER>/kmpolice/releases/download/v0.1.0/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "<SHA256_ARM64_MACOS>"
    else
      url "https://github.com/<ORG_OR_USER>/kmpolice/releases/download/v0.1.0/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "<SHA256_X64_MACOS>"
    end
  end

  def install
    bin.install "kmpolice"
  end

  test do
    output = shell_output("#{bin}/kmpolice --help")
    assert_match "Usage: kmpolice", output
  end
end
