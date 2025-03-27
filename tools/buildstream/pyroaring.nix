{pythonPkgs}:
pythonPkgs.buildPythonPackage rec {
  pname = "pyroaring";
  version = "1.0.0";

  src = pythonPkgs.fetchPypi {
    inherit pname version;
    hash = "sha256-rzdDTTuZHOXBZ/AZLTVnEoZoZk9MTxsS3b6Be4BETI0=";
  };

  # do not run tests
  doCheck = false;

  # specific to buildPythonPackage, see its reference
  pyproject = true;
  build-system = [
    pythonPkgs.setuptools
    pythonPkgs.cython
  ];
}
