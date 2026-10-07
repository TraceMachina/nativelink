import os
from pathlib import Path
import re
import sys

fsl_license = re.compile(r"""// Copyright \d{4}(?:-\d{4})? The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License \(the "License"\);
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

.+""", re.MULTILINE|re.DOTALL)

bsl_license = re.compile(r"""// Copyright \d{4} Trace Machina, Inc. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 \(the "License"\);
// you may not use this file except in compliance with the License.
// You may request a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

.+""", re.MULTILINE|re.DOTALL)

failed = False

root = sys.argv[1]
for dirstr, dirnames, filenames in os.walk(root):
    dirpath = Path(dirstr)
    if "target" in dirpath.parts:
        continue
    for filename in sorted(filenames):
        fullpath = dirpath.joinpath(filename)
        if fullpath.match("nativelink-util/src/fastcdc.rs"):
            # Different license
            continue
        rest, ext = os.path.splitext(filename)
        if ext == ".rs":
            with fullpath.open() as f:
                contents = f.read()
            if not fsl_license.match(contents) and not bsl_license.match(contents):
                print("%s didn't match expected license" % fullpath.relative_to(root))
                failed = True

if failed:
    sys.exit(1)
