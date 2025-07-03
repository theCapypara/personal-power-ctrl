{
  rustPlatform,
  pkg-config,
  openssl,
}:
rustPlatform.buildRustPackage rec {
  pname = "personal-power-ctrl";
  version = "0.2.0";

  src = "${../.}";

  cargoLock = {
    lockFile = "${src}/Cargo.lock";
    outputHashes = {
      "hs100api-0.1.1" = "sha256-edAfe2YbmhgU6aZFB+dpjBV8iQlUZzVXuIta7W6f4Pg=";
      "kodi-jsonrpc-client-0.1.0" = "sha256-pBIsSHF+/vrgn14lrDIp1XG9eQNxZVMAP+f1j+V7A40=";
    };
  };

  nativeBuildInputs = [
    pkg-config
  ];

  buildInputs = [
    openssl
  ];
}
