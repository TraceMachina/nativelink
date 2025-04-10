# TODO(palfrey): Replace with upstream now https://github.com/NixOS/nixpkgs/pull/383451 is in
{
  lib,
  fetchFromGitHub,
  pythonPkgs,
  pkgs,
  pyroaring,
}:
pythonPkgs.buildPythonApplication rec {
  pname = "buildstream";
  version = "2.4.1";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "apache";
    repo = "buildstream";
    tag = version;
    hash = "sha256-6a0VzYO5yj7EHvAb0xa4xZ0dgBKjFcwKv2F4o93oahY=";
  };

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
    description = "Powerful software integration tool";
    downloadPage = "https://buildstream.build/install.html";
    homepage = "https://buildstream.build";
    license = lib.licenses.asl20;
    platforms = lib.platforms.linux;
    mainProgram = "bst";
  };
}
