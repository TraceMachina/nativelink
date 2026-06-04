kind: manual
description: |

  Building manually

# Depend on the base system
depends:
- base.bst

# Stage the files/src directory for building
sources:
  - kind: local
    path: files/src

# Now configure the commands to run
config:
  build-commands:
  - echo hello.c
