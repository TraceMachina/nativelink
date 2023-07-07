#!/bin/bash

# This script is used only to make sure wrapper scripts work in
# running_actions_manager_test.rs.

# Print some static text to stderr. This is what the test uses to
# make sure the script did run.
>&2 printf "Wrapper script did run"

# Now run the real command.
exec "$@"
