import re
import subprocess
import requests
import sys
import pathlib

module_bazel_path = sys.argv[1]
cache_dir = pathlib.Path(__file__).parent.joinpath("cache")
cache_dir.mkdir(exist_ok=True)

original = open(module_bazel_path).read()
begin_shas = re.search("# BEGIN SHAS\n", original).end() # pyright: ignore[reportOptionalMemberAccess]
end_shas = re.search("\n        # END SHAS", original).start() # pyright: ignore[reportOptionalMemberAccess]
print(begin_shas, end_shas)
sha_pattern = re.compile(r"\"(.+\.tar\.xz)\": \"([0-9a-f]+)\"")

results = ""

for entry in sha_pattern.finditer(original, begin_shas, end_shas):
    short_url, hash = entry.groups()
    cache_path = cache_dir.joinpath(short_url.replace("/", "_"))
    if not cache_path.exists():
        full_url = f"https://static.rust-lang.org/dist/{short_url}"
        print("getting", full_url, cache_path)
        req = requests.get(full_url)
        with cache_path.open("wb") as f:
            f.write(req.content)
    sha256_cmd = subprocess.check_output(["sha256sum", cache_path.as_posix()], encoding="utf-8")
    sha256 = sha256_cmd.split(" ")[0]
    if results != "":
        results += "\n"
    results += f"        \"{short_url}\": \"{sha256}\","

revised = original[:begin_shas] + results + original[end_shas:]
with open(module_bazel_path, "w") as f:
    f.write(revised)
