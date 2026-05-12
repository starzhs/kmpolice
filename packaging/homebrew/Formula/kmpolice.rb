class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.1/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "fffa087389c744b4bbe7baad5fc88cff7dbbd46da4fc821d28b9fa0d05d6377f"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.1/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "95282508c4c224494c4d06e61679d3743e412db15996d0856761f8e347dc13ed"
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
