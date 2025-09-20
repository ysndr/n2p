{
  self,
  lib,
  rustPlatform,
}:
let
  cargoToml = lib.importTOML (self.outPath + "/Cargo.toml");

in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "n2p";
  version = cargoToml.package.version;

  src = self.outPath;

  cargoHash = "sha256-6JhLoK6tS8L4HJSBVO6V2GWszTZxM7Nh0PjVeF9Mi78=";

  meta = {
    description = cargoToml.package.description;
    homepage = cargoToml.package.homepage;
    license = lib.licenses.mit;
    maintainers = [ lib.maintainers.ysndr ];
  };
})
