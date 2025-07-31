{
  pkgs ? import <nixpkgs> { },
}:
pkgs.runCommand "hmm" { } ''
  echo ${builtins.currentSystem} >> $out

''
