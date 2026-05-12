class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.3/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "94d0d7cba40c0050beedc42ea5051c741cc2eb56cf3e83cedb8cdd9449d0865e"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.3/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "7c9bee2a442bada867f888eb904ac56763c1ad20829de17f97d96b9c056e6e89"
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
