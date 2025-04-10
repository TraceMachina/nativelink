# TODO(palfrey): Replace with upstream now https://github.com/NixOS/nixpkgs/pull/383451 is in
{
  pythonPkgs,
  fetchFromGitHub,
  lib,
}:
pythonPkgs.buildPythonPackage rec {
  pname = "pyroaring";
  version = "1.0.0";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "Ezibenroc";
    repo = "PyRoaringBitMap";
    tag = version;
    hash = "sha256-pnANvqyQ5DpG4NWSgWkAkXvSNLO67nfa97nEz3fYf1Y=";
  };

  doCheck = false;

  build-system = [
    pythonPkgs.setuptools
    pythonPkgs.cython
  ];

  meta = {
    description = "Python library for handling efficiently sorted integer sets";
    homepage = "https://github.com/Ezibenroc/PyRoaringBitMap";
    changelog = "https://github.com/Ezibenroc/PyRoaringBitMap/releases/tag/${version}";
    license = lib.licenses.mit;
  };
}
