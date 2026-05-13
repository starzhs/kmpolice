class Kmpolice < Formula
  desc "Static checker for Kotlin Multiplatform -> iOS Swift API impact"
  homepage "https://github.com/starzhs/kmpolice"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.5/kmpolice-aarch64-apple-darwin.tar.gz"
      sha256 "70c60e5a05245cd3ec5ca231f61dbff9cbbce8fbcdf1b755eefad828a0b505fd"
    else
      url "https://github.com/starzhs/kmpolice/releases/download/v0.1.5/kmpolice-x86_64-apple-darwin.tar.gz"
      sha256 "851dd713037036f308953b827c9a3e1e460509f85a4caa58a80f7fa21ec983aa"
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
