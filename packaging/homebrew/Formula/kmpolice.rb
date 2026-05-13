class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.6/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "18ef78f459628c6ecd3d9e5fed99144d26bc297bc70fd5578bf1fbc14fd4d8e2"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.6/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "b84f8dcb9345e54b7fec38f5541e1f9c94c6b94b9e6c3362ea721cec5437c1e8"
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
