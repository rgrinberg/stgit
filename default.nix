{ lib
, naersk
, stdenv
, targetPlatform
, pkg-config
, libiconv
, rustfmt
, cargo
, rustc
, pkgs
}:

let
  cargoToml = (builtins.fromTOML (builtins.readFile ./Cargo.toml));
in

naersk.lib."${targetPlatform.system}".buildPackage rec {
  src = ./.;

  buildInputs = [
    rustfmt
    # pkg-config
    cargo
    rustc
    pkgs.curl
    # pkgs.gpgme
    # pkgs.openssl
  ];
  checkInputs = [ cargo rustc ];

  doCheck = true;
  CARGO_BUILD_INCREMENTAL = "false";
  RUST_BACKTRACE = "full";
  copyLibs = true;

  name = cargoToml.package.name;
  version = cargoToml.package.version;

  meta = with lib; {
    # description = cargoToml.package.description;
    homepage = cargoToml.package.homepage;
    license = with licenses; [ gpl2 ];
    maintainers = with maintainers; [ ];
  };
}
