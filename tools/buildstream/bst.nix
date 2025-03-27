{
  pythonPkgs,
  pkgs,
  pyroaring,
}:
pythonPkgs.buildPythonApplication rec {
  pname = "buildstream";
  version = "2.4.1";

  src = pkgs.fetchPypi {
    inherit pname version;
    hash = "sha256-crjsC66S4L42dejUQQ+i9j4+W68JF9Dn6kezaN3EZF8=";
  };

  # do not run tests
  doCheck = false;

  dependencies = [
    pyroaring
    pythonPkgs.click
    pythonPkgs.grpcio
    pythonPkgs.jinja2
    pythonPkgs.pluginbase
    pythonPkgs.protobuf
    pythonPkgs.psutil
    pythonPkgs.ruamel-yaml
    pythonPkgs.ruamel-yaml-clib
    pythonPkgs.ujson
    pkgs.buildbox
  ];

  build-system = [
    pythonPkgs.setuptools
    pythonPkgs.cython
  ];

  meta = {
    mainProgram = "bst";
  };
}
