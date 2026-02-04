<!-- vale off -->
# Changelog

All notable changes to this project will be documented in this file.

## [0.8.0](https://github.com/TraceMachina/nativelink/compare/v0.7.10..0.8.0) - 2026-01-29

### ⛰️  Features

- Add additional logging around worker property matching ([#2118](https://github.com/TraceMachina/nativelink/issues/2118)) - ([24c637a](https://github.com/TraceMachina/nativelink/commit/24c637ab86b44864787bf7b789d6bf29b98df87f))

### 🐛 Bug Fixes

- Fix Redis index creation race ([#2111](https://github.com/TraceMachina/nativelink/issues/2111)) - ([c3a497d](https://github.com/TraceMachina/nativelink/commit/c3a497d36df49d3a1caadede02c4cc6d5af87492))

### 📚 Documentation

- Add docs for configuring Worker Match Logging Interval ([#2103](https://github.com/TraceMachina/nativelink/issues/2103)) - ([ae963be](https://github.com/TraceMachina/nativelink/commit/ae963be97178284a1aa53b526a3fa3292ca12e2a))

### 🧪 Testing & CI

- Every bytestream_read had a debug log, which we don't need ([#2117](https://github.com/TraceMachina/nativelink/issues/2117)) - ([18360ad](https://github.com/TraceMachina/nativelink/commit/18360ada6e5e3ecc04a7f6f96fbae09cf919111b))

### ⚙️ Miscellaneous

- output_files can be very noisy, drop from debug ([#2123](https://github.com/TraceMachina/nativelink/issues/2123)) - ([3ed406f](https://github.com/TraceMachina/nativelink/commit/3ed406faa9c116485218f1c5aa6340d5b9e312c4))
- Support ignorable platform properties ([#2120](https://github.com/TraceMachina/nativelink/issues/2120)) - ([1b45027](https://github.com/TraceMachina/nativelink/commit/1b450275c8d826c8124be121b62e61c67a2cad38))
- Reduce logging level for "Dropping file to update_file" ([#2116](https://github.com/TraceMachina/nativelink/issues/2116)) - ([95a8a34](https://github.com/TraceMachina/nativelink/commit/95a8a3438968ab082a38c343d708dd2a70ee74ed))
- Pull MAX_COUNT_PER_CURSOR into redis config, not hardcoding ([#2112](https://github.com/TraceMachina/nativelink/issues/2112)) - ([5b043eb](https://github.com/TraceMachina/nativelink/commit/5b043eb08ec46518db7784c6cfd9c47ae7fcc93d))
- Test redis improvements with client drop and higher max count per cursor ([#2110](https://github.com/TraceMachina/nativelink/issues/2110)) - ([bed6f9a](https://github.com/TraceMachina/nativelink/commit/bed6f9a8acf45da17fbd56d12202413360204218))

### ⬆️ Bumps & Version Updates

- *(deps)* update rust crate lru to 0.16.0 [security] ([#2106](https://github.com/TraceMachina/nativelink/issues/2106)) - ([c127bba](https://github.com/TraceMachina/nativelink/commit/c127bba823ca4e5df56da9eaa65df58787b74e3a))

## [0.7.10](https://github.com/TraceMachina/nativelink/compare/v0.7.9..v0.7.10) - 2025-12-29

### 🐛 Bug Fixes

- *(deps)* update module golang.org/x/crypto to v0.45.0 [security] ([#2062](https://github.com/TraceMachina/nativelink/issues/2062)) - ([7a4cdb6](https://github.com/TraceMachina/nativelink/commit/7a4cdb681fe23b90f68f1bcc897b5b9ce43c1e37))

### 🧪 Testing & CI

- New filesystem test for eviction breaking ([#2024](https://github.com/TraceMachina/nativelink/issues/2024)) - ([47ebd44](https://github.com/TraceMachina/nativelink/commit/47ebd44809657889f185d0cb36c4217012211c48))

### ⚙️ Miscellaneous

- *(deps)* update dependency abseil-cpp to v20250512 ([#2099](https://github.com/TraceMachina/nativelink/issues/2099)) - ([2bdb869](https://github.com/TraceMachina/nativelink/commit/2bdb869b7cb42ad1c2411f282d454fe2cb81cc65))
- *(deps)* update actions/checkout action to v6 ([#2085](https://github.com/TraceMachina/nativelink/issues/2085)) - ([fbda7bb](https://github.com/TraceMachina/nativelink/commit/fbda7bbfd1910bda6abace60feef3645f6f92ab4))
- *(deps)* update actions/github-script action to v8 ([#2098](https://github.com/TraceMachina/nativelink/issues/2098)) - ([f9f3b60](https://github.com/TraceMachina/nativelink/commit/f9f3b6031f400cb3ef327b2c956ea6c6d0d4ff54))
- reduce worker disconnect cascades ([#2093](https://github.com/TraceMachina/nativelink/issues/2093)) - ([44ada84](https://github.com/TraceMachina/nativelink/commit/44ada84405f17696c04f363b98773692a1c122f6))
- Replace rustls-pemfile to fix RUSTSEC-2025-0134 ([#2094](https://github.com/TraceMachina/nativelink/issues/2094)) - ([1b85f71](https://github.com/TraceMachina/nativelink/commit/1b85f71d977f61ff79391934e434af9c10d057e8))

## [0.7.9](https://github.com/TraceMachina/nativelink/compare/v0.7.8..v0.7.9) - 2025-12-10

### ⛰️  Features

- Add LazyNotFound Store Optimization, Support for fast_slow_store (S3, GCS slow_store targets) ([#2072](https://github.com/TraceMachina/nativelink/issues/2072)) - ([8c62bb3](https://github.com/TraceMachina/nativelink/commit/8c62bb318d849c7122659bd1c583fee627fa4f74))

### 🐛 Bug Fixes

- Fix the scheduler timeouts and errors ([#2083](https://github.com/TraceMachina/nativelink/issues/2083)) - ([93f4ead](https://github.com/TraceMachina/nativelink/commit/93f4eaddad157842549d1cd9cc1da676194997bd))

### ⚙️ Miscellaneous

- Perf spike ([#2081](https://github.com/TraceMachina/nativelink/issues/2081)) - ([422bfa1](https://github.com/TraceMachina/nativelink/commit/422bfa176891bae17eacb78f1b64e95bd68916d9))
- Implement remote execution metrics rebased ([#2080](https://github.com/TraceMachina/nativelink/issues/2080)) - ([e38af3d](https://github.com/TraceMachina/nativelink/commit/e38af3d6ce897084832fbd66757de25d532acae6))
- Build Custom Docker Image for each PR ([#2084](https://github.com/TraceMachina/nativelink/issues/2084)) - ([0926bff](https://github.com/TraceMachina/nativelink/commit/0926bffdf8918c9fd15b07673cb0cddab9c382ff))

## [0.7.8](https://github.com/TraceMachina/nativelink/compare/v0.7.7..v0.7.8) - 2025-11-27

### 🐛 Bug Fixes

- Use wildcard query when Redis index value is empty ([#2069](https://github.com/TraceMachina/nativelink/issues/2069))
- Fix assertion message for fastcdc ([#2056](https://github.com/TraceMachina/nativelink/issues/2056))
- Fix the changelog post 0.7.7 ([#2057](https://github.com/TraceMachina/nativelink/issues/2057))

### 🧪 Testing & CI

- Redis store tester and permits ([#1878](https://github.com/TraceMachina/nativelink/issues/1878))

### ⚙️ Miscellaneous

- *(deps)* Update dependency astro to v5.15.9 [security] ([#2061](https://github.com/TraceMachina/nativelink/issues/2061))
- Recoverable connection pool ([#2067](https://github.com/TraceMachina/nativelink/issues/2067))
- Revert "bugfix: prefix Redis index name and sort key ([#2066])" ([#2068](https://github.com/TraceMachina/nativelink/issues/2068))
- Prefix Redis index name and sort key ([#2066](https://github.com/TraceMachina/nativelink/issues/2066))
- Disable digest updates for renovate and Nix magic cache ([#2059](https://github.com/TraceMachina/nativelink/issues/2059))
- Do not need to store zero-length filesystem files ([#2033](https://github.com/TraceMachina/nativelink/issues/2033))
- Don't complain about worker stream error if we're shutting down ([#2055](https://github.com/TraceMachina/nativelink/issues/2055))

### ⬆️ Bumps & Version Updates

- Update the default max permits for redis ([#2063](https://github.com/TraceMachina/nativelink/issues/2063))

## [0.7.7](https://github.com/TraceMachina/nativelink/compare/v0.7.6..v0.7.7) - 2025-11-17



### ⛰️  Features

- Add periodic logging regarding scheduler job states ([#2042](https://github.com/TraceMachina/nativelink/issues/2042)) - ([7d6f663](https://github.com/TraceMachina/nativelink/commit/7d6f6632628df772289b76b21321bc3d25a230f8))

### 🧪 Testing & CI

- *(worker)* Resolve deadlock due to file permit exhaustion ([#2051](https://github.com/TraceMachina/nativelink/issues/2051)) ([#2052](https://github.com/TraceMachina/nativelink/issues/2052)) - ([b5dd8fb](https://github.com/TraceMachina/nativelink/commit/b5dd8fbaba59a47598189d49efce7e02fc0e9ed2))

### ⚙️ Miscellaneous

- *(deps)* update dependency astro to v5.15.6 [security] ([#2045](https://github.com/TraceMachina/nativelink/issues/2045)) - ([0cd70ee](https://github.com/TraceMachina/nativelink/commit/0cd70eebf7134b0102ae5d37eae825fc340e1bd5))

## [0.7.6](https://github.com/TraceMachina/nativelink/compare/v0.7.4..v0.7.6) - 2025-11-13



### ⛰️  Features

- Redo worker_find_logging as config ([#2039](https://github.com/TraceMachina/nativelink/issues/2039)) - ([958f687](https://github.com/TraceMachina/nativelink/commit/958f68763524e3f2d3d12f91e8949ecfeea98479))
- Log on command complete ([#2032](https://github.com/TraceMachina/nativelink/issues/2032)) - ([daea037](https://github.com/TraceMachina/nativelink/commit/daea03751c09e6553f3c9636003ad315811cec03))
- Directory Cache ([#2021](https://github.com/TraceMachina/nativelink/issues/2021)) - ([a01bd65](https://github.com/TraceMachina/nativelink/commit/a01bd652efb59cb092f1383398c54d694b137f60))
- Log failures to update actions ([#2022](https://github.com/TraceMachina/nativelink/issues/2022)) - ([3697512](https://github.com/TraceMachina/nativelink/commit/369751249eb19e8dc3bdbb31f041fa60c6948cbc))

### 🐛 Bug Fixes

- Fix flake timestamp ([#2036](https://github.com/TraceMachina/nativelink/issues/2036)) - ([e0e4d41](https://github.com/TraceMachina/nativelink/commit/e0e4d411e5942bd65d2ff864be2e7e0019dacc24))
- scheduler shutdown not guarded ([#2015](https://github.com/TraceMachina/nativelink/issues/2015)) - ([552a1cd](https://github.com/TraceMachina/nativelink/commit/552a1cde0013a90a9ceba93f77f4c18b6e475652))
- Fast slow store directions ([#1581](https://github.com/TraceMachina/nativelink/issues/1581)) - ([6d867c9](https://github.com/TraceMachina/nativelink/commit/6d867c99b08f6cb078900b5a9f4fae1e262158d9))

### 🧪 Testing & CI

- Add testing for running action manager failure logging ([#2031](https://github.com/TraceMachina/nativelink/issues/2031)) - ([922d7f6](https://github.com/TraceMachina/nativelink/commit/922d7f60b38dae49cf907217d8c1e485a011ced6))
- Fix fast store direction ([#2019](https://github.com/TraceMachina/nativelink/issues/2019)) - ([e7f29fe](https://github.com/TraceMachina/nativelink/commit/e7f29fe8aad6e2e6f7bef1ce822b983090d77fc2))
- Buck2 integration test ([#1828](https://github.com/TraceMachina/nativelink/issues/1828)) - ([1296a3a](https://github.com/TraceMachina/nativelink/commit/1296a3aaa6b1040d70f2d2609644698c57d029a6))

### ⚙️ Miscellaneous

- *(deps)* update swatinem/rust-cache digest to a84bfdc ([#2018](https://github.com/TraceMachina/nativelink/issues/2018)) - ([d5ea603](https://github.com/TraceMachina/nativelink/commit/d5ea603356adfa60e563af406429fdb836039173))
- Upgrade python3 to new security patch version ([#2044](https://github.com/TraceMachina/nativelink/issues/2044)) - ([222731d](https://github.com/TraceMachina/nativelink/commit/222731de0295abcdb9f6262cd5547d50168918cc))
- Use common_s3_utils in s3_store ([#2040](https://github.com/TraceMachina/nativelink/issues/2040)) - ([b2eaf79](https://github.com/TraceMachina/nativelink/commit/b2eaf79b19d3f12afa6194968cb582d466a2a0d6))
- Lockdown and upgrade the nix action versions ([#2038](https://github.com/TraceMachina/nativelink/issues/2038)) - ([f679946](https://github.com/TraceMachina/nativelink/commit/f6799465fc5a77263e025ffadeb6a670a9b37ffc))
- Log more info about redis key updates ([#2035](https://github.com/TraceMachina/nativelink/issues/2035)) - ([1d3cc10](https://github.com/TraceMachina/nativelink/commit/1d3cc10390b8c246f40dd675404a1b94a2122d58))
- Use display, not debug formatting for operation ids ([#2028](https://github.com/TraceMachina/nativelink/issues/2028)) - ([b7238b3](https://github.com/TraceMachina/nativelink/commit/b7238b3c1bbb549a7c364339d8a4b6e4a5d5ef47))
- Removes starter pricing ([#2027](https://github.com/TraceMachina/nativelink/issues/2027)) - ([bef18b3](https://github.com/TraceMachina/nativelink/commit/bef18b31024c1c612b1d995c524aff33b82d1390))
- Drops the cloud references ([#2025](https://github.com/TraceMachina/nativelink/issues/2025)) - ([c3431ac](https://github.com/TraceMachina/nativelink/commit/c3431acc109129586ee5a288166a5139e6a0d27c))
- Filestore update deadlock ([#2007](https://github.com/TraceMachina/nativelink/issues/2007)) - ([d55c59d](https://github.com/TraceMachina/nativelink/commit/d55c59dd101173195fde4376a6185cbaaa50d252))
- guard shutting down in scheduler while SIGTERM ([#2012](https://github.com/TraceMachina/nativelink/issues/2012)) - ([1708859](https://github.com/TraceMachina/nativelink/commit/17088593e5bcfc30f0e20cb9b25743ebcf90ca8b))
- Remove unnecessary Mutex ([#2006](https://github.com/TraceMachina/nativelink/issues/2006)) - ([083232d](https://github.com/TraceMachina/nativelink/commit/083232dc47946bdbba1f82b741ebf8dde3ac948e))

## [0.7.4](https://github.com/TraceMachina/nativelink/compare/v0.7.3..v0.7.4) - 2025-10-23



### ⛰️  Features

- GCS do not upload zero ([#1995](https://github.com/TraceMachina/nativelink/issues/1995)) - ([ab0d4e6](https://github.com/TraceMachina/nativelink/commit/ab0d4e6e1920f8d099ce17b8b20f93bbab6dba27))
- GCS store connect timeout ([#1994](https://github.com/TraceMachina/nativelink/issues/1994)) - ([854d51c](https://github.com/TraceMachina/nativelink/commit/854d51caddef98888eaaff3e5866a5248a482d67))
- Add cache to native-cargo step ([#1974](https://github.com/TraceMachina/nativelink/issues/1974)) - ([0c02306](https://github.com/TraceMachina/nativelink/commit/0c02306de8067c7f8d5c5d0e6b90c949ed3a99a6))
- Add metadata checks to machete ([#1952](https://github.com/TraceMachina/nativelink/issues/1952)) - ([21d5fdc](https://github.com/TraceMachina/nativelink/commit/21d5fdc3b5f5ce6cd99c3199b14c30a3a7774168))

### 🐛 Bug Fixes

- Fix clippy::cast_possible_truncation ([#1423](https://github.com/TraceMachina/nativelink/issues/1423)) - ([b050976](https://github.com/TraceMachina/nativelink/commit/b0509764084bd5aa1c6b61c39a63429f3c6b6859))
- Notify execution complete ([#1975](https://github.com/TraceMachina/nativelink/issues/1975)) - ([8527f25](https://github.com/TraceMachina/nativelink/commit/8527f258f756e5c337ad133dd635416bbf9b89fb))
- Fix removal state ([#1981](https://github.com/TraceMachina/nativelink/issues/1981)) - ([d85e491](https://github.com/TraceMachina/nativelink/commit/d85e491c4e26bd78d88d08c5d1ca357fc42b3e93))
- Fix Redis subscribe race ([#1970](https://github.com/TraceMachina/nativelink/issues/1970)) - ([9353508](https://github.com/TraceMachina/nativelink/commit/9353508fed8f96f5d754978047491869cbeba71a))

### 📚 Documentation

- fixed cost docs ([#1986](https://github.com/TraceMachina/nativelink/issues/1986)) - ([aab10ee](https://github.com/TraceMachina/nativelink/commit/aab10ee553781fb1bc2194d0eed58d6a625ee4f6))

### 🧪 Testing & CI

- Add Rust test to RBE work ([#1992](https://github.com/TraceMachina/nativelink/issues/1992)) - ([e01079b](https://github.com/TraceMachina/nativelink/commit/e01079b00f37c7211f5d2094c153e516dae09ef2))
- Make all tests in running_actions_manager_test serial ([#1984](https://github.com/TraceMachina/nativelink/issues/1984)) - ([41cdd9c](https://github.com/TraceMachina/nativelink/commit/41cdd9cd62ad431fff7dea2fdbab9252a55ae05c))
- comment legacy Dockerfile test ([#1983](https://github.com/TraceMachina/nativelink/issues/1983)) - ([6316b55](https://github.com/TraceMachina/nativelink/commit/6316b5529d3b228757ed454828352497caed39ea))
- Adds testing to bytestream backwards compatibility ([#1979](https://github.com/TraceMachina/nativelink/issues/1979)) - ([21bb502](https://github.com/TraceMachina/nativelink/commit/21bb502c1eae34900b461b43ad65a443deb95406))

### ⚙️ Miscellaneous

- Pin various dependencies (mostly Docker images) ([#1990](https://github.com/TraceMachina/nativelink/issues/1990)) - ([29c3dc4](https://github.com/TraceMachina/nativelink/commit/29c3dc4581e511d28f7355ca6d203ddc65394f0c))
- Unify all the service setups with a macro ([#1996](https://github.com/TraceMachina/nativelink/issues/1996)) - ([e46b5c7](https://github.com/TraceMachina/nativelink/commit/e46b5c7b8710df60efeaf895e9d92eb8296fc931))
- Sweep forgotten client operation IDs ([#1965](https://github.com/TraceMachina/nativelink/issues/1965)) - ([9fcf5b1](https://github.com/TraceMachina/nativelink/commit/9fcf5b1de4a8d7ac7623039f43d51d0682a65e67))
- Require default-features=false ([#1993](https://github.com/TraceMachina/nativelink/issues/1993)) - ([0146c34](https://github.com/TraceMachina/nativelink/commit/0146c34a6988a284c4b7d44ed4db14a2b66412e6))
- Single worker stream ([#1977](https://github.com/TraceMachina/nativelink/issues/1977)) - ([e9250ee](https://github.com/TraceMachina/nativelink/commit/e9250ee83296aaaf950a2d930bca9fa05cc2ad4a))
- Explicitly separate state locks and awaits ([#1991](https://github.com/TraceMachina/nativelink/issues/1991)) - ([930b352](https://github.com/TraceMachina/nativelink/commit/930b352548b1ca6a428e272d9c7ec12c2c228a2d))
- Replace derivative with derive_more ([#1989](https://github.com/TraceMachina/nativelink/issues/1989)) - ([9f39700](https://github.com/TraceMachina/nativelink/commit/9f397002214cc8d734624499de113c08c4178176))
- Build toolchain-examples ([#1971](https://github.com/TraceMachina/nativelink/issues/1971)) - ([2d08aba](https://github.com/TraceMachina/nativelink/commit/2d08abaeb9eaaa423eb3ebb598d0100a2212cf41))
- Remove folders with bad permissions ([#1980](https://github.com/TraceMachina/nativelink/issues/1980)) - ([5e487f3](https://github.com/TraceMachina/nativelink/commit/5e487f374d7ef2c13a0239aa37c4bfe963951f0e))
- Property replace ([#1976](https://github.com/TraceMachina/nativelink/issues/1976)) - ([41a2452](https://github.com/TraceMachina/nativelink/commit/41a2452ca0350eb6d153c6ac7b6af97c2152f614))
- Harden worker disconnect ([#1972](https://github.com/TraceMachina/nativelink/issues/1972)) - ([1055cd1](https://github.com/TraceMachina/nativelink/commit/1055cd150430769d043561f16f9c0b759e707dc4))
- Drop MacOS 14 support ([#1973](https://github.com/TraceMachina/nativelink/issues/1973)) - ([bdfa17c](https://github.com/TraceMachina/nativelink/commit/bdfa17c9c18439e7e20a0bdbddcda544e7110ebc))
- Drop 22.04 support ([#1883](https://github.com/TraceMachina/nativelink/issues/1883)) - ([4fe024b](https://github.com/TraceMachina/nativelink/commit/4fe024b03f118fa56842e0500fa190d32694396d))

### ⬆️ Bumps & Version Updates

- Update Swatinem/rust-cache digest to 9416228 ([#2004](https://github.com/TraceMachina/nativelink/issues/2004)) - ([15c747e](https://github.com/TraceMachina/nativelink/commit/15c747e056567bae86c0bfd8a153eb480d40d88a))
- Update dependency hermetic_cc_toolchain to v4 ([#1988](https://github.com/TraceMachina/nativelink/issues/1988)) - ([ed918d8](https://github.com/TraceMachina/nativelink/commit/ed918d8365a012c320a7cd8b4a0333975f2807ab))
- Update Rust crate relative-path to v2 ([#1985](https://github.com/TraceMachina/nativelink/issues/1985)) - ([997feb4](https://github.com/TraceMachina/nativelink/commit/997feb4537fa19f7e2cb3bfedc45f9add772ddcf))
- Update dependency astro to v5.14.3 [SECURITY] ([#1969](https://github.com/TraceMachina/nativelink/issues/1969)) - ([d896788](https://github.com/TraceMachina/nativelink/commit/d896788cda243950377a747c7e8c5b1cce1625d4))
- Update dependency dotenv to v17 ([#1966](https://github.com/TraceMachina/nativelink/issues/1966)) - ([3b7f05f](https://github.com/TraceMachina/nativelink/commit/3b7f05fce82a36e1339590b827bfee8cbe150221))

## [0.7.3](https://github.com/TraceMachina/nativelink/compare/v0.7.2..v0.7.3) - 2025-10-10



### ⛰️  Features

- Add timeout to health check ([#1961](https://github.com/TraceMachina/nativelink/issues/1961)) - ([cff9b6b](https://github.com/TraceMachina/nativelink/commit/cff9b6b58c32355278fdac855496e27a8880f06f))
- Detect anonymous GCS auth and optionally quit ([#1958](https://github.com/TraceMachina/nativelink/issues/1958)) - ([4b77932](https://github.com/TraceMachina/nativelink/commit/4b77932e8662fc3f1dfb4cfa44dcaaaea9e8ae2a))

### 🐛 Bug Fixes

- De-dupe the fast-slow store ([#1956](https://github.com/TraceMachina/nativelink/issues/1956)) - ([75f402c](https://github.com/TraceMachina/nativelink/commit/75f402c106d2a15739e04a7276b7de7058a8e674))
- Fix config parse control flow ([#1957](https://github.com/TraceMachina/nativelink/issues/1957)) - ([4d318c0](https://github.com/TraceMachina/nativelink/commit/4d318c09b8c5a07e492c054f680263a68b46d86e))

## [0.7.2](https://github.com/TraceMachina/nativelink/compare/v0.7.1..v0.7.2) - 2025-10-08



### ⛰️  Features

- Move Bytestream to array config ([#1951](https://github.com/TraceMachina/nativelink/issues/1951)) - ([e5b0eef](https://github.com/TraceMachina/nativelink/commit/e5b0eefe72d67b9364fb41c041cd5a0814a07582))
- Add more logging around active_drop_spawns ([#1941](https://github.com/TraceMachina/nativelink/issues/1941)) - ([24624ef](https://github.com/TraceMachina/nativelink/commit/24624effaa1930fa2f0d33dd36c53f770be95fdd))

### 🐛 Bug Fixes

- Fixes all the examples in the stores config ([#1948](https://github.com/TraceMachina/nativelink/issues/1948)) - ([f70c487](https://github.com/TraceMachina/nativelink/commit/f70c487da1875f1bdbfd2df6901d06883c0417c2))
- Prevent UUID collisions ([#1945](https://github.com/TraceMachina/nativelink/issues/1945)) - ([184d629](https://github.com/TraceMachina/nativelink/commit/184d6290743b6928dd573c59eb5b16b98b6c8d5d))
- Existence cache remove callbacks ([#1947](https://github.com/TraceMachina/nativelink/issues/1947)) - ([67adf59](https://github.com/TraceMachina/nativelink/commit/67adf590857017ed16f06a62248a074d10cd1ec5))
- Make the error on a size field clearer ([#1939](https://github.com/TraceMachina/nativelink/issues/1939)) - ([a294778](https://github.com/TraceMachina/nativelink/commit/a29477856efdb3c815d74626cea1de006561ccb6))

### 📚 Documentation

- added validation warnings ([#1938](https://github.com/TraceMachina/nativelink/issues/1938)) - ([068d095](https://github.com/TraceMachina/nativelink/commit/068d0957e0f150f46a341119142a8fbffcf76c56))

### ⚙️ Miscellaneous

- RHEL8 demo image ([#1933](https://github.com/TraceMachina/nativelink/issues/1933)) - ([e3b108f](https://github.com/TraceMachina/nativelink/commit/e3b108f26d76a15d61adb055e3a56c64c61bf41d))
- Better logging for store_awaited_action update failures ([#1940](https://github.com/TraceMachina/nativelink/issues/1940)) - ([892893e](https://github.com/TraceMachina/nativelink/commit/892893e1048a6d2b639fbacc62c8871319b128f5))
- update hero with trademark ([#1942](https://github.com/TraceMachina/nativelink/issues/1942)) - ([f5c2c17](https://github.com/TraceMachina/nativelink/commit/f5c2c17dfd87ed499688908ec8b6923ac4236436))
- LastMile AI case study ([#1937](https://github.com/TraceMachina/nativelink/issues/1937)) - ([ef03983](https://github.com/TraceMachina/nativelink/commit/ef039837078f626135d3695ebdec913889d660e0))
- Add trending badge ([#1936](https://github.com/TraceMachina/nativelink/issues/1936)) - ([969713d](https://github.com/TraceMachina/nativelink/commit/969713d60008558de8d16a74fa31ce4c1f8055bd))

## [0.7.1](https://github.com/TraceMachina/nativelink/compare/v0.7.0..v0.7.1) - 2025-09-24



### ⛰️  Features

- Add ONTAP S3 Store with existence cache ([#1630](https://github.com/TraceMachina/nativelink/issues/1630)) - ([b4c8216](https://github.com/TraceMachina/nativelink/commit/b4c82163190004a7469ed8a8d05680a59bc790d9))
- Add worker_find_logging ([#1925](https://github.com/TraceMachina/nativelink/issues/1925)) - ([8b46fd8](https://github.com/TraceMachina/nativelink/commit/8b46fd848b68a3c4a43c3f79fa9baef26eef9174))

### 🐛 Bug Fixes

- Extended license to FSL-Apache ([#1930](https://github.com/TraceMachina/nativelink/issues/1930)) - ([7fcee85](https://github.com/TraceMachina/nativelink/commit/7fcee85a0803958505431f310b23a07b558640a1))

### 🧪 Testing & CI

- Prepare `0.7.1` Release ([#1932](https://github.com/TraceMachina/nativelink/issues/1932)) - ([a36521e](https://github.com/TraceMachina/nativelink/commit/a36521ed342242c4bffef96406387e1afd6c790c))
- Re-enable integration tests ([#1915](https://github.com/TraceMachina/nativelink/issues/1915)) - ([3f9e037](https://github.com/TraceMachina/nativelink/commit/3f9e037428ccbdb3d427f89bf6f447a790d44de5))

### ⚙️ Miscellaneous

- Revert ExecutionComplete early scheduling optimization ([#1929](https://github.com/TraceMachina/nativelink/issues/1929)) - ([d39eeb6](https://github.com/TraceMachina/nativelink/commit/d39eeb625b8900f466894199aee38b707b850d82))
- Support pre-0.7.0 cacheable spelling ([#1926](https://github.com/TraceMachina/nativelink/issues/1926)) - ([32ef435](https://github.com/TraceMachina/nativelink/commit/32ef4350c2a017b57c149f4fb7546e2903efc6f7))
- Format JSON files ([#1927](https://github.com/TraceMachina/nativelink/issues/1927)) - ([ecc6c1e](https://github.com/TraceMachina/nativelink/commit/ecc6c1e85a63d48c97c9809abfd10d72b448b93a))
- Make the bazelrc warnings back to being actual warnings ([#1914](https://github.com/TraceMachina/nativelink/issues/1914)) - ([6180146](https://github.com/TraceMachina/nativelink/commit/6180146cd68d29feb16ef5863f42d56c63a68e5c))

### ⬆️ Bumps & Version Updates

- Update dependency astro to v5.13.2 [SECURITY] ([#1890](https://github.com/TraceMachina/nativelink/issues/1890)) - ([7010351](https://github.com/TraceMachina/nativelink/commit/7010351ac1a1ac7148508955c96b5a31536d7042))
- Update product pricing p2 ([#1923](https://github.com/TraceMachina/nativelink/issues/1923)) - ([7cedb68](https://github.com/TraceMachina/nativelink/commit/7cedb68e304c2cf0e19c2e3e460a2d66abfc41d2))
- Update the Nativelink pricing in the website ([#1921](https://github.com/TraceMachina/nativelink/issues/1921)) - ([e973aa1](https://github.com/TraceMachina/nativelink/commit/e973aa116b2bab6bdba915adedd66153172add83))
- Update Rust crate tracing-subscriber to v0.3.20 [SECURITY] ([#1917](https://github.com/TraceMachina/nativelink/issues/1917)) - ([f380d7d](https://github.com/TraceMachina/nativelink/commit/f380d7d112ebc292cfd78a6d99660d3ad650279e))

## [0.7.0](https://github.com/TraceMachina/nativelink/compare/v0.6.0..v0.7.0) - 2025-08-16



### ❌️  Breaking Changes

- [Breaking] Remove support for MacOS 13 on x86_64 ([#1732](https://github.com/TraceMachina/nativelink/issues/1732)) - ([d7deee3](https://github.com/TraceMachina/nativelink/commit/d7deee3332f0ca387d390710a15b0fd8c39af028))
- [Breaking] Change S3Store to a generic CloudObjectStore ([#1720](https://github.com/TraceMachina/nativelink/issues/1720)) - ([1d94417](https://github.com/TraceMachina/nativelink/commit/1d944178ec309fd97681688014a2ebc2e6d9969c))
- [Breaking] Remove backwards compatibility for configs ([#1695](https://github.com/TraceMachina/nativelink/issues/1695)) - ([aff81c8](https://github.com/TraceMachina/nativelink/commit/aff81c8b62c50e316614b55f9a2a7a39c6f9a577))
- [Breaking] Remove `experimental_prometheus` and `disable_metrics` ([#1686](https://github.com/TraceMachina/nativelink/issues/1686)) - ([23a64cf](https://github.com/TraceMachina/nativelink/commit/23a64cf1bfc97fe7bf0607983612f0625832fbf2))

### ⛰️  Features

- Early scheduling ([#1904](https://github.com/TraceMachina/nativelink/issues/1904)) - ([85c279a](https://github.com/TraceMachina/nativelink/commit/85c279a4467c5322159c5f55bca05be6b3bf92c4))
- CMake tutorial for C/C++ devs not using Bazel/Buck2 ([#1896](https://github.com/TraceMachina/nativelink/issues/1896)) - ([bc95749](https://github.com/TraceMachina/nativelink/commit/bc957491734752a7fbfc5f21265c14a3870af438))
- Add the O'Reilly book to our website ([#1886](https://github.com/TraceMachina/nativelink/issues/1886)) - ([d4e556d](https://github.com/TraceMachina/nativelink/commit/d4e556dde22c5405b930e2e7e55a3ba8b7eea711))
- Add nix command to create local nativelink images ([#1871](https://github.com/TraceMachina/nativelink/issues/1871)) - ([fb538ca](https://github.com/TraceMachina/nativelink/commit/fb538ca240c65f6444ce6ba9b898f421a85f1c87))
- Add debug helpers to check LRE kustomization ([#1857](https://github.com/TraceMachina/nativelink/issues/1857)) - ([ba1cd53](https://github.com/TraceMachina/nativelink/commit/ba1cd53033c9f9110afe9468446ff5ea52ef8b90))
- Add GCS bucket/path when we fail to load metadata ([#1849](https://github.com/TraceMachina/nativelink/issues/1849)) - ([b63f623](https://github.com/TraceMachina/nativelink/commit/b63f623eb3db8b093c8ac2682e8f7b156bc566e5))
- Add Browserbase wordmark ([#1800](https://github.com/TraceMachina/nativelink/issues/1800)) - ([bace8c0](https://github.com/TraceMachina/nativelink/commit/bace8c014516738d68fe76a9c70bd8b3524e20dc))
- Add a Contributors Section To Website's Home Page ([#1797](https://github.com/TraceMachina/nativelink/issues/1797)) - ([f39be28](https://github.com/TraceMachina/nativelink/commit/f39be28935880016de5bd4b76576c4fa0f3d79a8))
- Add support for native root certs ([#1782](https://github.com/TraceMachina/nativelink/issues/1782)) - ([cd3f993](https://github.com/TraceMachina/nativelink/commit/cd3f9930eac4942569b939c19f891f50e699cb00))
- Add typos to spellcheck source code ([#1780](https://github.com/TraceMachina/nativelink/issues/1780)) - ([d545a1c](https://github.com/TraceMachina/nativelink/commit/d545a1c265a4c931e091f931c3eaba3c43ed76f6))
- Add Finetuning LLMs on CPUs Blog Post ([#1748](https://github.com/TraceMachina/nativelink/issues/1748)) - ([18eb9e1](https://github.com/TraceMachina/nativelink/commit/18eb9e1d71c7d2b9ca2a1fbd71a545aaa1bf13ab))
- Add shellcheck to lint shell scripts ([#1759](https://github.com/TraceMachina/nativelink/issues/1759)) - ([ce06332](https://github.com/TraceMachina/nativelink/commit/ce06332381270ce0588c09c6e8126712395c1aa9))
- Add implementation for Google Cloud Storage with REST ([#1645](https://github.com/TraceMachina/nativelink/issues/1645)) - ([2839470](https://github.com/TraceMachina/nativelink/commit/2839470fc8cf04f9d0fd2aa228318d7b3d7d9827))
- Add ISA support for buildstream ([#1681](https://github.com/TraceMachina/nativelink/issues/1681)) - ([4b42fec](https://github.com/TraceMachina/nativelink/commit/4b42fecd3a25b64c790e9e5a53b3ed6bc4fab719))
- Add actualized param for reclient config dir ([#1679](https://github.com/TraceMachina/nativelink/issues/1679)) - ([39d390d](https://github.com/TraceMachina/nativelink/commit/39d390d1d680c16f58b7e02f9ab437ed461bc706))
- Add RemoteAsset protobuf ([#1647](https://github.com/TraceMachina/nativelink/issues/1647)) - ([07bba7c](https://github.com/TraceMachina/nativelink/commit/07bba7c9a9d824dd37240280af646076b427c023))
- Add Thirdwave Automation case study ([#1615](https://github.com/TraceMachina/nativelink/issues/1615)) - ([0125a34](https://github.com/TraceMachina/nativelink/commit/0125a347514682431f6886cdbd9e0f8cf6500eb7))

### 🐛 Bug Fixes

- Fix Docker error due to version drift ([#1882](https://github.com/TraceMachina/nativelink/issues/1882)) - ([3c9b1f3](https://github.com/TraceMachina/nativelink/commit/3c9b1f353c588c2d5a8ca1f6e35da37a510e8670))
- Fix directory collision on action retries by waiting for cleanup and removing stales ([#1868](https://github.com/TraceMachina/nativelink/issues/1868)) - ([47602d1](https://github.com/TraceMachina/nativelink/commit/47602d1d83e9e478a56fb3fbeaa5c5e1fee813f4))
- Fix local rustfmt with new flags ([#1850](https://github.com/TraceMachina/nativelink/issues/1850)) - ([efd5c5c](https://github.com/TraceMachina/nativelink/commit/efd5c5cb3e49df663537ce5f99d809adf9ea638f))
- Fix execution_server instance name error ([#1858](https://github.com/TraceMachina/nativelink/issues/1858)) - ([e362da8](https://github.com/TraceMachina/nativelink/commit/e362da828963a760b705425bbb361b61875e5f24))
- Fix wrong log messaging while removing file in `FilesystemStore` ([#1400](https://github.com/TraceMachina/nativelink/issues/1400)) - ([350070d](https://github.com/TraceMachina/nativelink/commit/350070de3317a03d1652f8bb8b20d735c8c6c3e8))
- Improve root cert blog post ([#1795](https://github.com/TraceMachina/nativelink/issues/1795)) - ([3ad3f20](https://github.com/TraceMachina/nativelink/commit/3ad3f20d91f8178132a15756605bf9530778537e))
- Fix blog post image. ([#1791](https://github.com/TraceMachina/nativelink/issues/1791)) - ([47fab25](https://github.com/TraceMachina/nativelink/commit/47fab25138db5d4bf03a0a6042aa4b2daa153ae9))
- Resolve `clippy::fallible_impl_from` ([#1771](https://github.com/TraceMachina/nativelink/issues/1771)) - ([d53363d](https://github.com/TraceMachina/nativelink/commit/d53363dca585e5a467fe38fef2c914928537b5c3))
- Fix clippy::similar_names ([#1777](https://github.com/TraceMachina/nativelink/issues/1777)) - ([acc2a8a](https://github.com/TraceMachina/nativelink/commit/acc2a8a50a2d857673acadd073439b02ddc2bcc0))
- Fix clippy::from_iter_instead_of_collect ([#1768](https://github.com/TraceMachina/nativelink/issues/1768)) - ([f281e9a](https://github.com/TraceMachina/nativelink/commit/f281e9a643dac25cd3f24a70d1d742dd8b5fa96a))
- Fix clippy::option_option ([#1765](https://github.com/TraceMachina/nativelink/issues/1765)) - ([1432b36](https://github.com/TraceMachina/nativelink/commit/1432b36b204432019764843a9e6114c5c710e87e))
- Fix clippy::unnecessary_semicolon ([#1769](https://github.com/TraceMachina/nativelink/issues/1769)) - ([4721a81](https://github.com/TraceMachina/nativelink/commit/4721a8190436046dfcf695416e09d8042f1ac0ff))
- Fix clippy::doc_link_with_quotes ([#1767](https://github.com/TraceMachina/nativelink/issues/1767)) - ([b52451a](https://github.com/TraceMachina/nativelink/commit/b52451ac940abe076ac4efc91101adaa209b6eb2))
- Fix clippy::if_not_else ([#1766](https://github.com/TraceMachina/nativelink/issues/1766)) - ([ea03da7](https://github.com/TraceMachina/nativelink/commit/ea03da78425857018c5095664d196da1f13fbeb9))
- Fix clippy lints after d106fe7 ([#1758](https://github.com/TraceMachina/nativelink/issues/1758)) - ([368bdb4](https://github.com/TraceMachina/nativelink/commit/368bdb48905d0adfb306506f7a12956cc0eb1b1b))
- Fix remote build against lre-rs on NixOS ([#1762](https://github.com/TraceMachina/nativelink/issues/1762)) - ([c86801a](https://github.com/TraceMachina/nativelink/commit/c86801a0117fe180eaa2f4a386e24e48bc7e6e13))
- Fix outdated homepage link ([#1755](https://github.com/TraceMachina/nativelink/issues/1755)) - ([ec4592b](https://github.com/TraceMachina/nativelink/commit/ec4592bcfbb1764c806c82e19de77f79d2c1d37f))
- Fix formatting in configuration-intro ([#1742](https://github.com/TraceMachina/nativelink/issues/1742)) - ([08f1eb0](https://github.com/TraceMachina/nativelink/commit/08f1eb0a1b988f6017e9b488cf1f6f9dc09c1b10))
- Handle slashes in instance name of `WaitExecutionRequest` ([#1689](https://github.com/TraceMachina/nativelink/issues/1689)) - ([5f4bbbf](https://github.com/TraceMachina/nativelink/commit/5f4bbbfa9adda750f9509d8e1c7dc6f47cceffcb))
- Remove console-subscriber ([#1683](https://github.com/TraceMachina/nativelink/issues/1683)) - ([3ba41c9](https://github.com/TraceMachina/nativelink/commit/3ba41c902fe3bd32cf1855d7742289ac4d1b8039))
- Fix admin router syntax for axum 0.8 ([#1675](https://github.com/TraceMachina/nativelink/issues/1675)) - ([3d8f4a8](https://github.com/TraceMachina/nativelink/commit/3d8f4a81763ef958e041e9e94362c73cef1723ed))
- Fix keyword casing in docker-compose Dockerfile ([#1663](https://github.com/TraceMachina/nativelink/issues/1663)) - ([c196ce4](https://github.com/TraceMachina/nativelink/commit/c196ce4506dda655fcdebf3124924899722c9c31))
- Fix various Bazel warnings after 24cbbfd501ffe5a569e23c2c456b391b58f4d8e4 ([#1621](https://github.com/TraceMachina/nativelink/issues/1621)) - ([742c985](https://github.com/TraceMachina/nativelink/commit/742c985a6fd08757045a70d463dfb8fb8ee537d7))

### 📚 Documentation

- Updating version in README and package manifests ([#1911](https://github.com/TraceMachina/nativelink/issues/1911)) - ([fe996ab](https://github.com/TraceMachina/nativelink/commit/fe996ab61dd26bcd13ff5c933efdbdadda841589))
- Migrate tracing infrastructure to OpenTelemetry ([#1772](https://github.com/TraceMachina/nativelink/issues/1772)) - ([7a8f561](https://github.com/TraceMachina/nativelink/commit/7a8f561aaa4a2336a6a42d45e87cbadbad284997))
- Add store README ([#1739](https://github.com/TraceMachina/nativelink/issues/1739)) - ([92ddb62](https://github.com/TraceMachina/nativelink/commit/92ddb62d3aa90132fbacb34a7bda2bae28471b9a))
- Refactor `write_too_many_bytes_fails` test ([#1726](https://github.com/TraceMachina/nativelink/issues/1726)) - ([a0c5db0](https://github.com/TraceMachina/nativelink/commit/a0c5db0afbfc26bae02bd76bc59915ea76a75cb0))
- Throw error on generate docs fail ([#1710](https://github.com/TraceMachina/nativelink/issues/1710)) - ([d9577c3](https://github.com/TraceMachina/nativelink/commit/d9577c3c5edf35cb5705913b9c306410af5ad0ef))
- Prepare development cluster for OpenTelemetry ([#1685](https://github.com/TraceMachina/nativelink/issues/1685)) - ([6811139](https://github.com/TraceMachina/nativelink/commit/6811139133a3c5fc203769a6a02777b43a3695db))
- Update ECR docs ([#1667](https://github.com/TraceMachina/nativelink/issues/1667)) - ([b09f9a6](https://github.com/TraceMachina/nativelink/commit/b09f9a6603763804ea6c156e8ddfca3b17d7972e))
- Update native-cli loadbalancer and flux ([#1670](https://github.com/TraceMachina/nativelink/issues/1670)) - ([665cca8](https://github.com/TraceMachina/nativelink/commit/665cca89cf103ab0f5b3f4fb204ff31e85d82441))
- Fix links in documentation ([#1655](https://github.com/TraceMachina/nativelink/issues/1655)) - ([8071565](https://github.com/TraceMachina/nativelink/commit/8071565cb2d7ff4978da191a8e6c900fc7f58fac))
- Document contributing to the native-cli ([#1625](https://github.com/TraceMachina/nativelink/issues/1625)) - ([4e3366d](https://github.com/TraceMachina/nativelink/commit/4e3366dd4d42e5d3ce4f2b69d541ddd3462af2a0))

### 🧪 Testing & CI

- Fake Redis test ([#1895](https://github.com/TraceMachina/nativelink/issues/1895)) - ([df93f97](https://github.com/TraceMachina/nativelink/commit/df93f97ebbe65921f2e4c89366b6dd0caedcd98b))
- Tested redaction for stream.first_msg in bytestream ([#1865](https://github.com/TraceMachina/nativelink/issues/1865)) - ([cd1e515](https://github.com/TraceMachina/nativelink/commit/cd1e51535f74d67a1e7ade08c38f2a00a421174a))
- Fix RBE testing ([#1862](https://github.com/TraceMachina/nativelink/issues/1862)) - ([4efa1ab](https://github.com/TraceMachina/nativelink/commit/4efa1ab98a9357b34b7e353733ed166b4b91e2df))
- Add integration test for mongo backend ([#1853](https://github.com/TraceMachina/nativelink/issues/1853)) - ([db1e341](https://github.com/TraceMachina/nativelink/commit/db1e341448dc88b25e370115629b59ccb10f140b))
- Add JSON5 formatting to pre-commit ([#1817](https://github.com/TraceMachina/nativelink/issues/1817)) - ([4616615](https://github.com/TraceMachina/nativelink/commit/4616615a4189d8096d7c0bac503b2ba48aa5590a))
- Re-enable doctests for nativelink-proto ([#1824](https://github.com/TraceMachina/nativelink/issues/1824)) - ([82b30ff](https://github.com/TraceMachina/nativelink/commit/82b30ff785d7e148e664c88e60707b6c5f393570))
- Make default config for k8s examples more realistic ([#1802](https://github.com/TraceMachina/nativelink/issues/1802)) - ([45e300c](https://github.com/TraceMachina/nativelink/commit/45e300c529908a5e59632d0bdda3ba499b2187ec))
- Largely switch from map-based to array-based config ([#1712](https://github.com/TraceMachina/nativelink/issues/1712)) - ([3f1cf3b](https://github.com/TraceMachina/nativelink/commit/3f1cf3b6340780bc68f45eb9482bcee8976e0048))
- Synchronize clippy lints between bazel and cargo ([#1745](https://github.com/TraceMachina/nativelink/issues/1745)) - ([1a61af2](https://github.com/TraceMachina/nativelink/commit/1a61af2acffa892fd2ac8de1f8cb0ffc1b507dd4))
- Add shfmt to lint shell scripts ([#1749](https://github.com/TraceMachina/nativelink/issues/1749)) - ([945c45c](https://github.com/TraceMachina/nativelink/commit/945c45c1aa94fd5fc558f28eb47f9bbe1af7f0e4))
- Test bytestream message too large ([#1721](https://github.com/TraceMachina/nativelink/issues/1721)) - ([3dc666c](https://github.com/TraceMachina/nativelink/commit/3dc666cb4da88aa30407771ff4bdc915c905f57b))
- Use default pre-commit hooks where possible ([#1723](https://github.com/TraceMachina/nativelink/issues/1723)) - ([e1d2e6f](https://github.com/TraceMachina/nativelink/commit/e1d2e6fa61a4fe7a2028c1f411ac30be5b33b602))
- Create Bazel flake template ([#1718](https://github.com/TraceMachina/nativelink/issues/1718)) - ([d95db0d](https://github.com/TraceMachina/nativelink/commit/d95db0dac1b196f2b35a8782eff782b27971c3a0))
- Add unit tests to bazel ([#1691](https://github.com/TraceMachina/nativelink/issues/1691)) - ([6473203](https://github.com/TraceMachina/nativelink/commit/6473203198f03aa4103c6b9ce1fc9c6af03a62c4))
- Resolve clippy lints, change to `#[expect]` ([#1661](https://github.com/TraceMachina/nativelink/issues/1661)) - ([8d97af7](https://github.com/TraceMachina/nativelink/commit/8d97af79d1fe7613d2e9b1548581605e03448043))

### ⚙️ Miscellaneous

- Prepare 0.7.0-rc-2 ([#1908](https://github.com/TraceMachina/nativelink/issues/1908)) - ([b23cf19](https://github.com/TraceMachina/nativelink/commit/b23cf19ce07f3415a82a4860641d7d6248a17bd6))
- Modified the todos, though many will be removed ([#1909](https://github.com/TraceMachina/nativelink/issues/1909)) - ([0e9626c](https://github.com/TraceMachina/nativelink/commit/0e9626cefa4f234db7938c2379ac3e5322171ce8))
- Retry matching on failure ([#1892](https://github.com/TraceMachina/nativelink/issues/1892)) - ([e691bea](https://github.com/TraceMachina/nativelink/commit/e691bea24ba0b0b5827e9464a26cfd8988b61512))
- Temporarily disable llre.yaml ([#1902](https://github.com/TraceMachina/nativelink/issues/1902)) - ([7c02e58](https://github.com/TraceMachina/nativelink/commit/7c02e589c6d0386db5e15487fd108a882fe97083))
- Graceful worker shutdown ([#1899](https://github.com/TraceMachina/nativelink/issues/1899)) - ([98b1201](https://github.com/TraceMachina/nativelink/commit/98b1201433e3e7834dc4d1d1a2d8688061a26047))
- Improve visibility of .conf ([#1900](https://github.com/TraceMachina/nativelink/issues/1900)) - ([d196648](https://github.com/TraceMachina/nativelink/commit/d1966487a3fafd29e178aa183c265c124c582c9f))
- Typo/makefile formatting ([#1897](https://github.com/TraceMachina/nativelink/issues/1897)) - ([de2abb8](https://github.com/TraceMachina/nativelink/commit/de2abb8a929cadac9688820bd1f1eda4a1ddc447))
- Repository hygiene, Rust 1.89.0, enter to submit ([#1894](https://github.com/TraceMachina/nativelink/issues/1894)) - ([e2cb612](https://github.com/TraceMachina/nativelink/commit/e2cb612037f613a26042932d322cd5d1fba4699b))
- Download work on submit ([#1893](https://github.com/TraceMachina/nativelink/issues/1893)) - ([052c53a](https://github.com/TraceMachina/nativelink/commit/052c53a543934c58c28661419e5f795d0064815d))
- Improve hero consistency ([#1887](https://github.com/TraceMachina/nativelink/issues/1887)) - ([d7ec1e1](https://github.com/TraceMachina/nativelink/commit/d7ec1e157a6e6340a5f44a7baeff9a5bfa59b06b))
- Redact data fields in tracing ([#1884](https://github.com/TraceMachina/nativelink/issues/1884)) - ([bee59b5](https://github.com/TraceMachina/nativelink/commit/bee59b5206b21175db49ab99190fb41f7154404d))
- Make Redis connection errors actually fail as such ([#1879](https://github.com/TraceMachina/nativelink/issues/1879)) - ([4e2c20e](https://github.com/TraceMachina/nativelink/commit/4e2c20e7dd75caa6d67b88e6ba4d57963bb79c21))
- Create the client-to-operation mapping when a  client subscribes to an existing action ([#1876](https://github.com/TraceMachina/nativelink/issues/1876)) - ([7caa78b](https://github.com/TraceMachina/nativelink/commit/7caa78bea5bd0e1f59cbfcaeb4b5cfa68b1a3eba))
- Improve evicting map performance ([#1875](https://github.com/TraceMachina/nativelink/issues/1875)) - ([036e394](https://github.com/TraceMachina/nativelink/commit/036e394838f08c79abafdc3f65926b602faf8dce))
- When logging errors, detail the keys ([#1877](https://github.com/TraceMachina/nativelink/issues/1877)) - ([eeec964](https://github.com/TraceMachina/nativelink/commit/eeec9643e0dcb042f2d282bdd2ecc5e5a3d44339))
- Readd publish-ghcr as needed by deploy ([#1873](https://github.com/TraceMachina/nativelink/issues/1873)) - ([0a331e5](https://github.com/TraceMachina/nativelink/commit/0a331e54c0dc68ff76d562c0bcde7fd0a9a436f3))
- Redis scheduler store should read OperationId as a JSON instead of String. ([#1872](https://github.com/TraceMachina/nativelink/issues/1872)) - ([7ee11d6](https://github.com/TraceMachina/nativelink/commit/7ee11d657b65586ca09880474654ce79a09bd497))
- Backwards compatibility now says what to change ([#1870](https://github.com/TraceMachina/nativelink/issues/1870)) - ([0c006fd](https://github.com/TraceMachina/nativelink/commit/0c006fdab5f709b6c92ded0bbed6c3d41cf7d572))
- Reduce confusion ([#1867](https://github.com/TraceMachina/nativelink/issues/1867)) - ([6aaee38](https://github.com/TraceMachina/nativelink/commit/6aaee38747d35281644704fe4360cb9ff4b8a445))
- Re-add Nix magic cache ([#1851](https://github.com/TraceMachina/nativelink/issues/1851)) - ([8d9470b](https://github.com/TraceMachina/nativelink/commit/8d9470b711c30acaa33db09bb549a5faac489fc1))
- Log fallback calls to help with adding new gRPC bits ([#1861](https://github.com/TraceMachina/nativelink/issues/1861)) - ([05bef36](https://github.com/TraceMachina/nativelink/commit/05bef36519a44ca734e0dc16a44118e44bca67d6))
- Remove background video on mobile ([#1812](https://github.com/TraceMachina/nativelink/issues/1812)) - ([181e39d](https://github.com/TraceMachina/nativelink/commit/181e39d6edb766a40f53baacc371e15236750ac4))
- Remove unused cargo deps with machete ([#1839](https://github.com/TraceMachina/nativelink/issues/1839)) - ([5a11bce](https://github.com/TraceMachina/nativelink/commit/5a11bce8ac9a79106f2f388915d89512e0313968))
- Mark all warnings as errors so bazel fails ([#1840](https://github.com/TraceMachina/nativelink/issues/1840)) - ([e6cf730](https://github.com/TraceMachina/nativelink/commit/e6cf730efdbb8a137d00ad61176f4d5858f03518))
- Reduce renovate noise by limiting to security and major fixes only ([#1836](https://github.com/TraceMachina/nativelink/issues/1836)) - ([a24fa5b](https://github.com/TraceMachina/nativelink/commit/a24fa5b47f28d531736485a5014a0d3127b1cfe2))
- Remove trace level and add note ([#1805](https://github.com/TraceMachina/nativelink/issues/1805)) - ([91ee900](https://github.com/TraceMachina/nativelink/commit/91ee9002b59f43c2b3dfaaf9b3e89c0c83500601))
- Don't allow used_underscore_binding ([#1819](https://github.com/TraceMachina/nativelink/issues/1819)) - ([e70a4bb](https://github.com/TraceMachina/nativelink/commit/e70a4bb42ff04dc2ebff0afa54be3c104da20369))
- Make config references version-specific ([#1823](https://github.com/TraceMachina/nativelink/issues/1823)) - ([cd73302](https://github.com/TraceMachina/nativelink/commit/cd733021c16c2112a48bcf36bd3a1bace453fbe0))
- Override the reclient ToC with a working version ([#1827](https://github.com/TraceMachina/nativelink/issues/1827)) - ([36ccefd](https://github.com/TraceMachina/nativelink/commit/36ccefd6d023fd9e599bccd4919da3d6fe95d838))
- Check example JSON5 files pass the parser ([#1818](https://github.com/TraceMachina/nativelink/issues/1818)) - ([20ad6a3](https://github.com/TraceMachina/nativelink/commit/20ad6a3e79f1959dbf815e5ba572a6910632b3b0))
- Implements the internals of the remote asset protocol ([#1816](https://github.com/TraceMachina/nativelink/issues/1816)) - ([4a299f9](https://github.com/TraceMachina/nativelink/commit/4a299f9f38a4e15065c807f66d6336415a46e82c))
- Generate bazel lints from Cargo.toml ([#1820](https://github.com/TraceMachina/nativelink/issues/1820)) - ([1cd0e5c](https://github.com/TraceMachina/nativelink/commit/1cd0e5c3f25cbcf8ff0491c69702cf5d1c221867))
- Replace Video for Website's Hero Section ([#1809](https://github.com/TraceMachina/nativelink/issues/1809)) - ([9b4fbd4](https://github.com/TraceMachina/nativelink/commit/9b4fbd473f4cdd070243b0b823a405ba4887b8c3))
- Use upstream buildstream packaging ([#1815](https://github.com/TraceMachina/nativelink/issues/1815)) - ([58513f3](https://github.com/TraceMachina/nativelink/commit/58513f3bc2ef22f785c9ba3b4e1b66242dc025bf))
- Modify blog image ([#1811](https://github.com/TraceMachina/nativelink/issues/1811)) - ([afc36bd](https://github.com/TraceMachina/nativelink/commit/afc36bd55087ab2c782dd696d65a38a3108ad926))
- Include Vale changes into web only workflows ([#1793](https://github.com/TraceMachina/nativelink/issues/1793)) - ([5c87e88](https://github.com/TraceMachina/nativelink/commit/5c87e88df180f46e7bc19eec66e6827166feae0a))
- Use native root certs for S3 stores ([#1785](https://github.com/TraceMachina/nativelink/issues/1785)) - ([44e35ba](https://github.com/TraceMachina/nativelink/commit/44e35baaf40b6c27e8173f77f43d8449d6a94df0))
- Blog about trust root support ([#1788](https://github.com/TraceMachina/nativelink/issues/1788)) - ([0fec68e](https://github.com/TraceMachina/nativelink/commit/0fec68eb4fdfc58a7425c415e4c76886cfc2c0fd))
- Reduce verbosity of the info trace level ([#1778](https://github.com/TraceMachina/nativelink/issues/1778)) - ([fe813a9](https://github.com/TraceMachina/nativelink/commit/fe813a96a443a92decd1c5139739257d63f417a8))
- Move redis fingerprint logic to logs ([#1773](https://github.com/TraceMachina/nativelink/issues/1773)) - ([708ab5b](https://github.com/TraceMachina/nativelink/commit/708ab5b311339b735dc29d5689f70227e8cdb1a5))
- Simplify clippy configs ([#1764](https://github.com/TraceMachina/nativelink/issues/1764)) - ([c66ead2](https://github.com/TraceMachina/nativelink/commit/c66ead2158b420d44143c38ff14e8862bd0b254b))
- Remove python from NixOS path ([#1763](https://github.com/TraceMachina/nativelink/issues/1763)) - ([19d4aac](https://github.com/TraceMachina/nativelink/commit/19d4aacdd5efa536859b78c7f12c6a7301cd0405))
- Make K8s filesystem paths independent of `$HOME` ([#1761](https://github.com/TraceMachina/nativelink/issues/1761)) - ([c31233e](https://github.com/TraceMachina/nativelink/commit/c31233e914e10d8bbc9d7afaee5f900f48885e39))
- Change title on website ([#1760](https://github.com/TraceMachina/nativelink/issues/1760)) - ([5be8d25](https://github.com/TraceMachina/nativelink/commit/5be8d25cf4b3cccf4a177072a0a0de3a8f03f3ac))
- Enable more clippy lints ([#1746](https://github.com/TraceMachina/nativelink/issues/1746)) - ([d106fe7](https://github.com/TraceMachina/nativelink/commit/d106fe711a65b9e2180003f0fca385894e0c47be))
- Test stream termination ([#1741](https://github.com/TraceMachina/nativelink/issues/1741)) - ([f9ab7c4](https://github.com/TraceMachina/nativelink/commit/f9ab7c437d0a50c5cceee4b4568d4a403fd09051))
- Disable unnecessary workflows for web changes ([#1750](https://github.com/TraceMachina/nativelink/issues/1750)) - ([36d1c43](https://github.com/TraceMachina/nativelink/commit/36d1c4364f3b698a8123ec7023dd233eb51dfc08))
- Reassign TODOs ([#1747](https://github.com/TraceMachina/nativelink/issues/1747)) - ([03152f1](https://github.com/TraceMachina/nativelink/commit/03152f1b6d274567fe85167bc7ce1c8990de8067))
- Remove unnecessary photos ([#1733](https://github.com/TraceMachina/nativelink/issues/1733)) - ([411a018](https://github.com/TraceMachina/nativelink/commit/411a01808c31b3dfc292cc9b812a47dce40652a5))
- Format toml files with taplo ([#1724](https://github.com/TraceMachina/nativelink/issues/1724)) - ([f6269d1](https://github.com/TraceMachina/nativelink/commit/f6269d19f392a90a7a63e9b9d3835d84f04868cd))
- Implement `StoreDriver::list` for `RedisStore` ([#1697](https://github.com/TraceMachina/nativelink/issues/1697)) - ([06362d5](https://github.com/TraceMachina/nativelink/commit/06362d5014e767bdc07aaf24508b9fa96969ae6d))
- Use explicit level macros instead of events ([#1725](https://github.com/TraceMachina/nativelink/issues/1725)) - ([78247a2](https://github.com/TraceMachina/nativelink/commit/78247a219def0296e6e4e17f792780499750574d))
- Rename name to path in rustdoc ([#1708](https://github.com/TraceMachina/nativelink/issues/1708)) - ([8f327d7](https://github.com/TraceMachina/nativelink/commit/8f327d734685e33e7bbfaf9b09195e7f60863eaa))
- Use `alloc`, `core` when possible ([#1704](https://github.com/TraceMachina/nativelink/issues/1704)) - ([18572ab](https://github.com/TraceMachina/nativelink/commit/18572ab3598fa70e965aa5371b5421d6b4489d36))
- Refactor flake modules ([#1699](https://github.com/TraceMachina/nativelink/issues/1699)) - ([f9ff630](https://github.com/TraceMachina/nativelink/commit/f9ff630e09a3c22d6a3abea68d1bacc775eac6bb))
- Initial Remote Asset support ([#1646](https://github.com/TraceMachina/nativelink/issues/1646)) - ([d319fda](https://github.com/TraceMachina/nativelink/commit/d319fdae798bc4cfbdce2fcf051b7d1b878644d4))
- Standardize flake naming conventions ([#1698](https://github.com/TraceMachina/nativelink/issues/1698)) - ([0ff64b1](https://github.com/TraceMachina/nativelink/commit/0ff64b10796a4612644e234e1181c836adb59981))
- Ramp up linting ([#1672](https://github.com/TraceMachina/nativelink/issues/1672)) - ([840a5b3](https://github.com/TraceMachina/nativelink/commit/840a5b36224a1727048719512fc0a75ab5adc1cc))
- Refactor K8s namespaces ([#1680](https://github.com/TraceMachina/nativelink/issues/1680)) - ([0419f76](https://github.com/TraceMachina/nativelink/commit/0419f7629071b5fdf0a4eeecd6fab64883c5280c))
- Ensure soundness of, rename `RawSymbolWrapper` ([#1673](https://github.com/TraceMachina/nativelink/issues/1673)) - ([9122f19](https://github.com/TraceMachina/nativelink/commit/9122f1945641e11d87fcb204dc4934343062c2f0))
- Rename variants to Rust standards ([#1666](https://github.com/TraceMachina/nativelink/issues/1666)) - ([12b24be](https://github.com/TraceMachina/nativelink/commit/12b24be141c8d852a827242c2cd51dd0d934d957))
- Remove indirection for wrapping tonic error codes ([#1656](https://github.com/TraceMachina/nativelink/issues/1656)) - ([a204116](https://github.com/TraceMachina/nativelink/commit/a204116e0a71c45d640187cbe32630efb16c4340))
- Remove redundant settings in `Cargo.toml` ([#1659](https://github.com/TraceMachina/nativelink/issues/1659)) - ([3cff6ac](https://github.com/TraceMachina/nativelink/commit/3cff6acdb8f89ad89baa3d36db8bcef9ca995cdd))
- Adjust nofile limit recommendations ([#1641](https://github.com/TraceMachina/nativelink/issues/1641)) - ([3431126](https://github.com/TraceMachina/nativelink/commit/343112689999ac39a27a2c53bb74397fb7e78723))
- Migrate S3Store to hyper 1.x ([#1639](https://github.com/TraceMachina/nativelink/issues/1639)) - ([a5e845c](https://github.com/TraceMachina/nativelink/commit/a5e845ce3d41832f158ecf91ab3598921ba5ae75))
- Start cilium before capacitor ([#1644](https://github.com/TraceMachina/nativelink/issues/1644)) - ([f91871c](https://github.com/TraceMachina/nativelink/commit/f91871cf64fb05b5ea2fd6fe24340188d59ad12f))
- Use selector function for stdenv ([#1642](https://github.com/TraceMachina/nativelink/issues/1642)) - ([6952c3e](https://github.com/TraceMachina/nativelink/commit/6952c3e39fbe690d7b091fb3fd772d1dab017e85))
- Migrate to Bazel 8 ([#1618](https://github.com/TraceMachina/nativelink/issues/1618)) - ([24cbbfd](https://github.com/TraceMachina/nativelink/commit/24cbbfd501ffe5a569e23c2c456b391b58f4d8e4))
- Adjust team to show leaders ([#1617](https://github.com/TraceMachina/nativelink/issues/1617)) - ([fa64033](https://github.com/TraceMachina/nativelink/commit/fa6403351287e51e0e7b7f70613626a578723b8f))

### ⬆️ Bumps & Version Updates

- Retry on disconnect ([#1906](https://github.com/TraceMachina/nativelink/issues/1906)) - ([ea0e0ae](https://github.com/TraceMachina/nativelink/commit/ea0e0ae3927af505fc16b73af78ef306c9314118))
- Update company.tsx ([#1901](https://github.com/TraceMachina/nativelink/issues/1901)) - ([1354bb0](https://github.com/TraceMachina/nativelink/commit/1354bb03d10d7009b596a897d3fe27bcf458469d))
- Upgrades Mongo library to 3.x ([#1854](https://github.com/TraceMachina/nativelink/issues/1854)) - ([739613b](https://github.com/TraceMachina/nativelink/commit/739613b1a7d001da00a0acb2a46d5d8470383cd2))
- Update ubuntu:22.04 Docker digest to 3c61d37 ([#1025](https://github.com/TraceMachina/nativelink/issues/1025)) - ([add1637](https://github.com/TraceMachina/nativelink/commit/add16372c9b919a653e55f54d19ce2394b6b8194))
- Fix GCS store implementation ([#1846](https://github.com/TraceMachina/nativelink/issues/1846)) - ([3d2dd5e](https://github.com/TraceMachina/nativelink/commit/3d2dd5e6d1ef3d95ed2f5d060a8044729c98e74f))
- Add ExperimentalMongoStore ([#1807](https://github.com/TraceMachina/nativelink/issues/1807)) - ([bc1c5ce](https://github.com/TraceMachina/nativelink/commit/bc1c5ce2c1f2d60a9e9f3b5b8f3c59e0e13d5d14))
- Update dependency toolchains_protoc to v0.4.3 ([#1833](https://github.com/TraceMachina/nativelink/issues/1833)) - ([8c6180c](https://github.com/TraceMachina/nativelink/commit/8c6180cec2c5039bb30e63ef2b4b97abaf7fc5a9))
- Bump github.com/cloudflare/circl from 1.6.0 to 1.6.1 in /native-cli ([#1834](https://github.com/TraceMachina/nativelink/issues/1834)) - ([da0f87f](https://github.com/TraceMachina/nativelink/commit/da0f87f0d1ea85fd2edf668aa3871a8c4c99ce2d))
- Update Rust crate formatx to v0.2.4 ([#1751](https://github.com/TraceMachina/nativelink/issues/1751)) - ([5aebecd](https://github.com/TraceMachina/nativelink/commit/5aebecdd136b3c93424153fa44cee6859be5c471))
- Update dependency rules_rust to v0.61.0 ([#1650](https://github.com/TraceMachina/nativelink/issues/1650)) - ([de0e26f](https://github.com/TraceMachina/nativelink/commit/de0e26fde7e537d391613c180ff2901b86a9dae6))
- Updates smithy to remove proc-macro-error ([#1822](https://github.com/TraceMachina/nativelink/issues/1822)) - ([6e9b131](https://github.com/TraceMachina/nativelink/commit/6e9b131410d7fa5d05aa1cd52ba22e20089ebd95))
- Update nix setup for GHA workflows ([#1813](https://github.com/TraceMachina/nativelink/issues/1813)) - ([76e769c](https://github.com/TraceMachina/nativelink/commit/76e769cd5ec067c443b56f5da417534c62865892))
- Update bincode to 2.0.1 ([#1803](https://github.com/TraceMachina/nativelink/issues/1803)) - ([dd5d19c](https://github.com/TraceMachina/nativelink/commit/dd5d19c20d2df94429107fe45b46242f079f914c))
- Update team ([#1801](https://github.com/TraceMachina/nativelink/issues/1801)) - ([5aa3603](https://github.com/TraceMachina/nativelink/commit/5aa3603db46d59381f769109f426ea639665a4a4))
- Bump flake ([#1783](https://github.com/TraceMachina/nativelink/issues/1783)) - ([88e14dc](https://github.com/TraceMachina/nativelink/commit/88e14dc03a1d49d956b9712a1a88f6076d09ad7b))
- Update website hero ([#1776](https://github.com/TraceMachina/nativelink/issues/1776)) - ([8a81bde](https://github.com/TraceMachina/nativelink/commit/8a81bde8148b5c227f1ddf8e2f29a5366ae209e5))
- Fix various website issues ([#1752](https://github.com/TraceMachina/nativelink/issues/1752)) - ([9287f6d](https://github.com/TraceMachina/nativelink/commit/9287f6def51a8b4f63aeb2ed1155ae1238292315))
- Update dependency @builder.io/qwik to v1.13.0 ([#1735](https://github.com/TraceMachina/nativelink/issues/1735)) - ([d6acccf](https://github.com/TraceMachina/nativelink/commit/d6acccf0c0df8d3cca09168d9719292f67d82368))
- Update configuration example "stores" field format ([#1727](https://github.com/TraceMachina/nativelink/issues/1727)) - ([9798a0d](https://github.com/TraceMachina/nativelink/commit/9798a0d36eca489e3c9d8df7fb4a180f61b8e393))
- Upgrade to 2024 edition ([#1676](https://github.com/TraceMachina/nativelink/issues/1676)) - ([07534c5](https://github.com/TraceMachina/nativelink/commit/07534c579b497e916f825e6cf43f4d2a92af7285))
- Update Rust crate tokio to v1.44.2 ([#1677](https://github.com/TraceMachina/nativelink/issues/1677)) - ([81b2c14](https://github.com/TraceMachina/nativelink/commit/81b2c14118bd549764fea47e759ac297ecc47296))
- Update Rust dependencies ([#1674](https://github.com/TraceMachina/nativelink/issues/1674)) - ([6b0cb60](https://github.com/TraceMachina/nativelink/commit/6b0cb60050ecab5c0ba944d7ef17635d91bb87d3))
- Bump flake ([#1671](https://github.com/TraceMachina/nativelink/issues/1671)) - ([1cc2baf](https://github.com/TraceMachina/nativelink/commit/1cc2bafdbbcf25873ac673bc53d1036212fe875b))
- Update website nits ([#1658](https://github.com/TraceMachina/nativelink/issues/1658)) - ([1982938](https://github.com/TraceMachina/nativelink/commit/198293884e399b48953826d55eb5aa6c97a67b2a))
- Bump flake ([#1632](https://github.com/TraceMachina/nativelink/issues/1632)) - ([07bd27a](https://github.com/TraceMachina/nativelink/commit/07bd27a7b28aea8b21bcc8a2eca547ce7771c2fa))
- Bump Cilium to 1.17.2 ([#1631](https://github.com/TraceMachina/nativelink/issues/1631)) - ([403a71c](https://github.com/TraceMachina/nativelink/commit/403a71c458f34a0b396af3a88f8609e4390b371a))
- Bump Go deps ([#1622](https://github.com/TraceMachina/nativelink/issues/1622)) - ([c72adee](https://github.com/TraceMachina/nativelink/commit/c72adee4f791cd76eeeccdeed7165a5ad568c957))
- Bump AWS SDK for Rust ([#1620](https://github.com/TraceMachina/nativelink/issues/1620)) - ([e465f73](https://github.com/TraceMachina/nativelink/commit/e465f7315a3f62cf8495a8567bdf5781d175402f))

## [0.6.0](https://github.com/TraceMachina/nativelink/compare/v0.5.4..v0.6.0) - 2025-03-06



### ❌️  Breaking Changes

- [Breaking] Remove ResumableFileSlot and rely on high ulimits ([#1582](https://github.com/TraceMachina/nativelink/issues/1582)) - ([8b89c31](https://github.com/TraceMachina/nativelink/commit/8b89c311f5c0a64bc9a755fdb9937b4ed54ba9c6))

### ⛰️  Features

- Add Grpc, Memory & S3 store to health checker registry ([#1586](https://github.com/TraceMachina/nativelink/issues/1586)) - ([44d8db1](https://github.com/TraceMachina/nativelink/commit/44d8db10259aafa622c26d6f27ce312a53edcfc0))
- Add ability to prefix worker_id in config ([#1578](https://github.com/TraceMachina/nativelink/issues/1578)) - ([e753b8d](https://github.com/TraceMachina/nativelink/commit/e753b8d4dc84711fe8b656690ce9890ccc2e85c9))
- Add OriginEvent for scheduler scheduling action ([#1574](https://github.com/TraceMachina/nativelink/issues/1574)) - ([60b0049](https://github.com/TraceMachina/nativelink/commit/60b0049e505481fbfc8a2644bf25a9dca37d3258))

### 🐛 Bug Fixes

- Move Tekton from Pulumi to Flux ([#1593](https://github.com/TraceMachina/nativelink/issues/1593)) - ([96adea4](https://github.com/TraceMachina/nativelink/commit/96adea4479431ecb9b77cc517b07a51a6b1e2d63))
- GrpcStore now sends digest function from context ([#1587](https://github.com/TraceMachina/nativelink/issues/1587)) - ([fc85156](https://github.com/TraceMachina/nativelink/commit/fc851567305d9b20837ecb7b27ea8212ff4a2061))

### 📚 Documentation

- Remove unused document file ([#1388](https://github.com/TraceMachina/nativelink/issues/1388)) - ([48c12b9](https://github.com/TraceMachina/nativelink/commit/48c12b9aa0ec55af371ef6f0af30a198e1d6e1a6))

### 🧪 Testing & CI

- Chage remote exec CI to new endpoints ([#1601](https://github.com/TraceMachina/nativelink/issues/1601)) - ([d755d30](https://github.com/TraceMachina/nativelink/commit/d755d301121ecf50ee748e5ef4bc26310655a1d2))
- Upgrade rand crate version and stabilize test rand generation ([#1583](https://github.com/TraceMachina/nativelink/issues/1583)) - ([79c2357](https://github.com/TraceMachina/nativelink/commit/79c2357fd2732b6fe6d0bee2aa49486f8758d43e))
- ClientKeepAlive update action ClientKeepAlive ([#1580](https://github.com/TraceMachina/nativelink/issues/1580)) - ([7afe286](https://github.com/TraceMachina/nativelink/commit/7afe2868313395d844ea6751667d1e0fd4987fc9))

### ⚙️ Miscellaneous

- Remove GrpcStore from health checker registry ([#1602](https://github.com/TraceMachina/nativelink/issues/1602)) - ([cba7359](https://github.com/TraceMachina/nativelink/commit/cba7359cc03d43789e2fa0b9cea634bc3d2c4900))
- Mark functions `const` where possible ([#1573](https://github.com/TraceMachina/nativelink/issues/1573)) - ([8b9824f](https://github.com/TraceMachina/nativelink/commit/8b9824fea7b77b5e45838649ceff5d2aaa46c365))
- Remove atime references to FilesystemStore ([#1584](https://github.com/TraceMachina/nativelink/issues/1584)) - ([0d6cbed](https://github.com/TraceMachina/nativelink/commit/0d6cbedeae514224c710fd736b9d6a03b571a5d2))
- ensuring everything is scrubbed. ([#1576](https://github.com/TraceMachina/nativelink/issues/1576)) - ([a8c7339](https://github.com/TraceMachina/nativelink/commit/a8c73395e95619cb07c8506c7f29c95a8ac7f7d1))

### ⬆️ Bumps & Version Updates

- Update readme ([#1611](https://github.com/TraceMachina/nativelink/issues/1611)) - ([1e5d866](https://github.com/TraceMachina/nativelink/commit/1e5d86602a9161452a52db72a2bfa8fca07c1118))
- Bump Go deps ([#1603](https://github.com/TraceMachina/nativelink/issues/1603)) - ([284eeb2](https://github.com/TraceMachina/nativelink/commit/284eeb20891aba7edd122db0137872d1f592494c))
- Bump flake ([#1596](https://github.com/TraceMachina/nativelink/issues/1596)) - ([34f1c94](https://github.com/TraceMachina/nativelink/commit/34f1c94e9cd2b4340b08b397805efd30a564574b))
- Refactor GitHub actions ([#1589](https://github.com/TraceMachina/nativelink/issues/1589)) - ([f11c88b](https://github.com/TraceMachina/nativelink/commit/f11c88b01356c27a140a52ca6d8419a0524e1b9b))

## [0.5.4](https://github.com/TraceMachina/nativelink/compare/v0.5.3..v0.5.4) - 2025-01-30



### ⛰️  Features

- Add `Closed` stream event to OriginEvents ([#1570](https://github.com/TraceMachina/nativelink/issues/1570)) - ([2d2986b](https://github.com/TraceMachina/nativelink/commit/2d2986b81307b827dcd375a99258d8a6922de363))
- Add ananonymized blog ([#1567](https://github.com/TraceMachina/nativelink/issues/1567)) - ([90c086b](https://github.com/TraceMachina/nativelink/commit/90c086b64e69fbab1de47c230638c35a9030ed0e))
- Add Aaron's awesome talk to homepage and resource page ([#1452](https://github.com/TraceMachina/nativelink/issues/1452)) - ([0915e03](https://github.com/TraceMachina/nativelink/commit/0915e03a0cc24142072ae7f57ff84740956e236d))
- Add event type info to node_id info in UUID ([#1550](https://github.com/TraceMachina/nativelink/issues/1550)) - ([b1df876](https://github.com/TraceMachina/nativelink/commit/b1df876fd64d60d5d1b6cb15a50e934923ab82bf))
- Add OriginEventPublisher ([#1497](https://github.com/TraceMachina/nativelink/issues/1497)) - ([f280e71](https://github.com/TraceMachina/nativelink/commit/f280e71cc08364307e79199ac64ca9185418f69c))
- Add google-cloud-sdk to flake ([#1526](https://github.com/TraceMachina/nativelink/issues/1526)) - ([d75d20d](https://github.com/TraceMachina/nativelink/commit/d75d20d524ff2c39714e669cfe530e28150facc8))
- Introduce the LRE flake overlay ([#1516](https://github.com/TraceMachina/nativelink/issues/1516)) - ([ae71bc8](https://github.com/TraceMachina/nativelink/commit/ae71bc8d31533492e37ed0b6d058564e2611dc66))
- Add tekton operator to local dev cluster ([#1337](https://github.com/TraceMachina/nativelink/issues/1337)) - ([56dcd10](https://github.com/TraceMachina/nativelink/commit/56dcd10e24074d1a26ead5ae623d110f05c39639))
- Add ShutdownGuard to replace oneshot for shutdown ([#1491](https://github.com/TraceMachina/nativelink/issues/1491)) - ([a8c3217](https://github.com/TraceMachina/nativelink/commit/a8c32178bd1ad765a4e765c248f2ad756c44da48))
- Adds Analytics Container to Website. ([#1465](https://github.com/TraceMachina/nativelink/issues/1465)) - ([cb9d441](https://github.com/TraceMachina/nativelink/commit/cb9d4414ab1d6d088f9247e6aedbc72c1bcc1949))
- Add static content from s3 bucket ([#1440](https://github.com/TraceMachina/nativelink/issues/1440)) - ([3e8dc29](https://github.com/TraceMachina/nativelink/commit/3e8dc29b50a29713ee648e55a775fb6af073af65))
- Add graceful shutdown to worker instances ([#1394](https://github.com/TraceMachina/nativelink/issues/1394)) - ([d0eb00c](https://github.com/TraceMachina/nativelink/commit/d0eb00c88f73be7cf2e8ee157bf84c9246f73c1c))
- Add NixOS support ([#1287](https://github.com/TraceMachina/nativelink/issues/1287)) - ([b2386fd](https://github.com/TraceMachina/nativelink/commit/b2386fdd16ccc4d3330fcf91f593c7e9262a6197))
- [Bug fix] Adds retry logic to redis store ([#1407](https://github.com/TraceMachina/nativelink/issues/1407)) - ([a815ba0](https://github.com/TraceMachina/nativelink/commit/a815ba0cb781a2ddc5d2afd4c97ef676326311c0))
- Revert "Allow nativelink flake module to upload results ([#1369](https://github.com/TraceMachina/nativelink/issues/1369))" ([#1372](https://github.com/TraceMachina/nativelink/issues/1372)) - ([73dbf59](https://github.com/TraceMachina/nativelink/commit/73dbf59c9cd341aabd6c69578a4398e2fde54278))
- Allow nativelink flake module to upload results ([#1369](https://github.com/TraceMachina/nativelink/issues/1369)) - ([9600839](https://github.com/TraceMachina/nativelink/commit/9600839bd2ba0a6915908c55fca24f373c3a2106))
- Add pulumi k8s await functionality ([#1353](https://github.com/TraceMachina/nativelink/issues/1353)) - ([dfe821c](https://github.com/TraceMachina/nativelink/commit/dfe821c3c4a8ecb714d7e6812674b12ac128859f))
- [Feature] Add Redis Scheduler ([#1343](https://github.com/TraceMachina/nativelink/issues/1343)) - ([a6c3a6f](https://github.com/TraceMachina/nativelink/commit/a6c3a6fcca7ee7956db6fbbab77b9cafc2898af7))
- Add StoreAwaitedActionDb API ([#1342](https://github.com/TraceMachina/nativelink/issues/1342)) - ([ac4ca57](https://github.com/TraceMachina/nativelink/commit/ac4ca57bdf95401fcb170708d1bcae543790f748))
- Allow empty page_token for getTree ([#1340](https://github.com/TraceMachina/nativelink/issues/1340)) - ([d66d418](https://github.com/TraceMachina/nativelink/commit/d66d4188ae15ace3e58721aa0d3062f2d0a01b31))
- Add KeepAlive updating to ApiWorkerScheduler ([#1310](https://github.com/TraceMachina/nativelink/issues/1310)) - ([37ebd58](https://github.com/TraceMachina/nativelink/commit/37ebd58f204432e2e8bcdc6338e312874e16148c))

### 🐛 Bug Fixes

- Fix bug where actions rarely get timedout on rejoin ([#1569](https://github.com/TraceMachina/nativelink/issues/1569)) - ([41d2670](https://github.com/TraceMachina/nativelink/commit/41d267051da0bd0d11ef7c84ef1c52b14117b240))
- Fix broken Slack link ([#1557](https://github.com/TraceMachina/nativelink/issues/1557)) - ([1ee61b1](https://github.com/TraceMachina/nativelink/commit/1ee61b1a10daf9a51227cd4f238034cf47c5ca03))
- Fix clippy::implicit_hasher ([#1503](https://github.com/TraceMachina/nativelink/issues/1503)) - ([fdd163a](https://github.com/TraceMachina/nativelink/commit/fdd163aa083dbbc626f3df562bc98d79df204c89))
- Fix clippy::struct_field_names ([#1505](https://github.com/TraceMachina/nativelink/issues/1505)) - ([91f3a2c](https://github.com/TraceMachina/nativelink/commit/91f3a2c65122b0671340bc549d6532f94e6a26b4))
- Fix clippy::doc_markdown ([#1504](https://github.com/TraceMachina/nativelink/issues/1504)) - ([524dc11](https://github.com/TraceMachina/nativelink/commit/524dc1198883f9f622a6519ad93b6a7285c19b23))
- Fix clippy::{ignored_unit_patterns, needless_continue} ([#1502](https://github.com/TraceMachina/nativelink/issues/1502)) - ([5e5b170](https://github.com/TraceMachina/nativelink/commit/5e5b1707ec72a04484a4f5af80b307231a6b2208))
- Fix clippy::default_trait_access ([#1500](https://github.com/TraceMachina/nativelink/issues/1500)) - ([cbc86c6](https://github.com/TraceMachina/nativelink/commit/cbc86c6dbd78fd4f23bb5f7d9ac08d7e1db5aef0))
- Fix broken video link ([#1488](https://github.com/TraceMachina/nativelink/issues/1488)) - ([22707d7](https://github.com/TraceMachina/nativelink/commit/22707d766ee8979195573b43c23ce84179ef597b))
- Fix clippy::needless_raw_string_hashes ([#1473](https://github.com/TraceMachina/nativelink/issues/1473)) - ([545793c](https://github.com/TraceMachina/nativelink/commit/545793c1899cb899c4b4239b83051a741621a9a0))
- Fix clippy::ptr_as_ptr ([#1472](https://github.com/TraceMachina/nativelink/issues/1472)) - ([1cf6365](https://github.com/TraceMachina/nativelink/commit/1cf636523f6117ae43d055226627302f9ead7a0d))
- Fix clippy::stable_sort_primitive ([#1396](https://github.com/TraceMachina/nativelink/issues/1396)) - ([de372f7](https://github.com/TraceMachina/nativelink/commit/de372f79f90b190fe737ab5f1bfbd2362112531c))
- Fix clippy::explicit_into_iter_loop ([#1457](https://github.com/TraceMachina/nativelink/issues/1457)) - ([ac44984](https://github.com/TraceMachina/nativelink/commit/ac44984e8806107f9e2d1975442ecd56d01eaf9d))
- Fix clippy::items_after_statements ([#1456](https://github.com/TraceMachina/nativelink/issues/1456)) - ([7d0e6af](https://github.com/TraceMachina/nativelink/commit/7d0e6af622970f875704ef324056e50e5b3b2ce6))
- Correctly wait for LRE/Remote tekton pipelines ([#1455](https://github.com/TraceMachina/nativelink/issues/1455)) - ([070485f](https://github.com/TraceMachina/nativelink/commit/070485f5068abc62548afdfdbf7fc54efe983dd5))
- Fix clippy::explicit_iter_loop ([#1453](https://github.com/TraceMachina/nativelink/issues/1453)) - ([973f210](https://github.com/TraceMachina/nativelink/commit/973f210285593b8166375d0893c07f95ab288186))
- Work around trivy ratelimits ([#1442](https://github.com/TraceMachina/nativelink/issues/1442)) - ([b4cb577](https://github.com/TraceMachina/nativelink/commit/b4cb577a35f95e0ba81c19450a1ff1da1fdaaef0))
- Fix LRE/Remote workflow after b44383f ([#1441](https://github.com/TraceMachina/nativelink/issues/1441)) - ([399e95b](https://github.com/TraceMachina/nativelink/commit/399e95b65256dae47bfa1e846d575b5bd966edf2))
- Fix clippy::match_same_arms ([#1433](https://github.com/TraceMachina/nativelink/issues/1433)) - ([51a2fd4](https://github.com/TraceMachina/nativelink/commit/51a2fd42e372fb8c80051bdb241213bb347fe7c4))
- Fix misspellings in code files ([#1420](https://github.com/TraceMachina/nativelink/issues/1420)) - ([6899467](https://github.com/TraceMachina/nativelink/commit/68994678d1ac018828ad51559ea49d1de3c03465))
- Fix clippy::return_self_not_must_use ([#1435](https://github.com/TraceMachina/nativelink/issues/1435)) - ([6fcb3bb](https://github.com/TraceMachina/nativelink/commit/6fcb3bb32df1b2728d8066103a49c0723ce77edc))
- Fix clippy::redundant_else ([#1432](https://github.com/TraceMachina/nativelink/issues/1432)) - ([6ed0455](https://github.com/TraceMachina/nativelink/commit/6ed0455478c3fba3412be878c538673509484346))
- Fix clippy::inline_always ([#1431](https://github.com/TraceMachina/nativelink/issues/1431)) - ([4948580](https://github.com/TraceMachina/nativelink/commit/4948580021acd422dffa6da92184bc4a3378803e))
- Fix clippy::ref_as_ptr ([#1430](https://github.com/TraceMachina/nativelink/issues/1430)) - ([1887337](https://github.com/TraceMachina/nativelink/commit/1887337bc9c16e988f90346e3f62355c2bb8e3ed))
- Fix clippy::map_unwrap_or ([#1415](https://github.com/TraceMachina/nativelink/issues/1415)) - ([cf4f11d](https://github.com/TraceMachina/nativelink/commit/cf4f11d100966e6ce517bffddfd6a2ab03eeefc4))
- Fix clippy::cast_lossless ([#1426](https://github.com/TraceMachina/nativelink/issues/1426)) - ([9e5a145](https://github.com/TraceMachina/nativelink/commit/9e5a145a3274cf6030df7160dbb65f82a296fdb5))
- Fix clippy::unnecessary_wraps ([#1409](https://github.com/TraceMachina/nativelink/issues/1409)) - ([e3c2a58](https://github.com/TraceMachina/nativelink/commit/e3c2a5873c229be263ede3d1a828e2eb5a79b70d))
- Fix clippy::trivially_copy_pass_by_ref ([#1416](https://github.com/TraceMachina/nativelink/issues/1416)) - ([4aa69c2](https://github.com/TraceMachina/nativelink/commit/4aa69c2b030e1cca4b20715e34e6f953a050dbd3))
- Fix clippy::explicit_deref_methods ([#1410](https://github.com/TraceMachina/nativelink/issues/1410)) - ([f7ff342](https://github.com/TraceMachina/nativelink/commit/f7ff342073ba42091d078fd3277190fc02b43c2a))
- Fix LRE Remote Workflow ([#1424](https://github.com/TraceMachina/nativelink/issues/1424)) - ([e14732f](https://github.com/TraceMachina/nativelink/commit/e14732fad821734c050bca68daf38d2f5b7032b9))
- Fix clippy::needless_pass_by_value ([#1413](https://github.com/TraceMachina/nativelink/issues/1413)) - ([712608c](https://github.com/TraceMachina/nativelink/commit/712608ccd91a088545b9e93b7faf1f48355c7c18))
- Fix broken demo button link ([#1404](https://github.com/TraceMachina/nativelink/issues/1404)) - ([f5de318](https://github.com/TraceMachina/nativelink/commit/f5de31840116e1a27b77a16d638dce86c5c59614))
- Fix clippy::implicit_clone ([#1384](https://github.com/TraceMachina/nativelink/issues/1384)) - ([4001d12](https://github.com/TraceMachina/nativelink/commit/4001d12501e7a97cec67e03743cba21d1e91a62f))
- Fix clippy::match_wildcard_for_single_variants ([#1411](https://github.com/TraceMachina/nativelink/issues/1411)) - ([257aedb](https://github.com/TraceMachina/nativelink/commit/257aedba5c4e89ec00a04c8c51d2deb2e7ab134a))
- Fix clippy::inconsistent_struct_constructor ([#1412](https://github.com/TraceMachina/nativelink/issues/1412)) - ([85904fb](https://github.com/TraceMachina/nativelink/commit/85904fb045059f5e0db5c60e0ab13bcb4cec6b39))
- Fix clippy::range_plus_one ([#1395](https://github.com/TraceMachina/nativelink/issues/1395)) - ([8dfb0ae](https://github.com/TraceMachina/nativelink/commit/8dfb0ae2bf8c40c9398cb188263484ae0f12f834))
- Handle empty file request on dedup store ([#1398](https://github.com/TraceMachina/nativelink/issues/1398)) - ([fc6f155](https://github.com/TraceMachina/nativelink/commit/fc6f1558703d19c47bbac00ec71ee96c0e37afaa))
- Fix clippy::unreadable_literal ([#1392](https://github.com/TraceMachina/nativelink/issues/1392)) - ([d418132](https://github.com/TraceMachina/nativelink/commit/d4181325d8ce7951c2a54edad3678c3328413fe6))
- Fix clippy::semicolon_if_nothing_returned ([#1393](https://github.com/TraceMachina/nativelink/issues/1393)) - ([553f33c](https://github.com/TraceMachina/nativelink/commit/553f33c682d849020ca9e407c1a6c47cc49bc598))
- Fix S3Store retry might cause poisoned data ([#1383](https://github.com/TraceMachina/nativelink/issues/1383)) - ([e6eb5f7](https://github.com/TraceMachina/nativelink/commit/e6eb5f775135a02d77f78d16237739f79eccac61))
- Fix clippy::redundant_closure_for_method_calls ([#1380](https://github.com/TraceMachina/nativelink/issues/1380)) - ([2b24ce2](https://github.com/TraceMachina/nativelink/commit/2b24ce28f60ccc6d219f3de8945c4bc1ce0ce1ed))
- Fix clippy::single_match_else ([#1379](https://github.com/TraceMachina/nativelink/issues/1379)) - ([255e0e7](https://github.com/TraceMachina/nativelink/commit/255e0e7372997f950aa3dc4d2017a543ba498eaa))
- Fix clippy::manual_let_else ([#1361](https://github.com/TraceMachina/nativelink/issues/1361)) - ([3e8b0b1](https://github.com/TraceMachina/nativelink/commit/3e8b0b14bc19b1acf0d10eeedae401aa0fc07976))
- Fix the date on the case studies. ([#1357](https://github.com/TraceMachina/nativelink/issues/1357)) - ([b770b13](https://github.com/TraceMachina/nativelink/commit/b770b13f225827c55b24a6a92d82e6a199613eb4))
- Fix a possible infinite loop in `RedisStore::update` ([#1269](https://github.com/TraceMachina/nativelink/issues/1269)) - ([8d957a5](https://github.com/TraceMachina/nativelink/commit/8d957a5d25a3f27051a270c4db24682e55213ee5))
- Fix format issues in markdown files ([#1332](https://github.com/TraceMachina/nativelink/issues/1332)) - ([0ab5a99](https://github.com/TraceMachina/nativelink/commit/0ab5a9933beeb4033756b49c602a4e59b0c86f03))

### 📚 Documentation

- Create docs and examples for classic remote execution ([#1498](https://github.com/TraceMachina/nativelink/issues/1498)) - ([3f3d4e2](https://github.com/TraceMachina/nativelink/commit/3f3d4e2820aa88b82e6214cc8c1c2166005a5694))
- Fix Broken Links on docs/introduction/on-prem ([#1480](https://github.com/TraceMachina/nativelink/issues/1480)) - ([481226b](https://github.com/TraceMachina/nativelink/commit/481226be52a84ad5a6b990cc48e9f97512d8ccd2))
- Add Matomo tracking pixel to rest of public READMEs ([#1460](https://github.com/TraceMachina/nativelink/issues/1460)) - ([1157a04](https://github.com/TraceMachina/nativelink/commit/1157a043fde2f079cf871b5c3397a1d80b2a2d96))
- Introduce the NativeLink Kubernetes operator ([#1088](https://github.com/TraceMachina/nativelink/issues/1088)) - ([b44383f](https://github.com/TraceMachina/nativelink/commit/b44383fe16c2ae5d054d5ce66499a4ea897e9dae))
- Remove wildcard searching in redis scheduler ([#1408](https://github.com/TraceMachina/nativelink/issues/1408)) - ([2238ef9](https://github.com/TraceMachina/nativelink/commit/2238ef95005bee7e22b22a369275561587bec072))
- Fix `docs.nativelink.com` based URL not working ([#1386](https://github.com/TraceMachina/nativelink/issues/1386)) - ([d602746](https://github.com/TraceMachina/nativelink/commit/d6027465332a467772858746d2f4bc245055f289))
- Introduce nativelink web platform including docs & website ([#1285](https://github.com/TraceMachina/nativelink/issues/1285)) - ([0e8811f](https://github.com/TraceMachina/nativelink/commit/0e8811f5f06d1c3bbdf771b1a06c9dca52e3f17f))
- Update README.md with newest version ([#1351](https://github.com/TraceMachina/nativelink/issues/1351)) - ([51974db](https://github.com/TraceMachina/nativelink/commit/51974db7cd6882ea6d6ec82eebdad0c0962ff95b))
- Update docs for RBE exec properties to support GPU etc. ([#1350](https://github.com/TraceMachina/nativelink/issues/1350)) - ([0ccaa15](https://github.com/TraceMachina/nativelink/commit/0ccaa15c9bc1735e9bceb8dcd5128d7dc1e1f732))
- Update `docs` generation ([#1280](https://github.com/TraceMachina/nativelink/issues/1280)) - ([f337391](https://github.com/TraceMachina/nativelink/commit/f337391c4de0331d372c1780b4735f160d6bd2cf))
- Update Cloud RBE docs for private image repositories and advanced config ([#1333](https://github.com/TraceMachina/nativelink/issues/1333)) - ([a1191f2](https://github.com/TraceMachina/nativelink/commit/a1191f2760cd586dbaaa8a84d9e3b6860161c569))
- Update RBE docs for private image repositories ([#1324](https://github.com/TraceMachina/nativelink/issues/1324)) - ([3d8766f](https://github.com/TraceMachina/nativelink/commit/3d8766fffc13221f573d2d63ac8f14cddd6c9a75))
- Update cloud docs for RBE and Read Only ([#1322](https://github.com/TraceMachina/nativelink/issues/1322)) - ([96db0cb](https://github.com/TraceMachina/nativelink/commit/96db0cbbe7616ec4949578722773179555e278d1))
- Disable various test for docs only PRs ([#1323](https://github.com/TraceMachina/nativelink/issues/1323)) - ([065029b](https://github.com/TraceMachina/nativelink/commit/065029b481c6f41c889973bedfec2bd59130a4c3))

### 🧪 Testing & CI

- Fix hardcoded value in local-image-test ([#1545](https://github.com/TraceMachina/nativelink/issues/1545)) - ([f672af7](https://github.com/TraceMachina/nativelink/commit/f672af7d79ed8ab60e0b7f703aa625cba528e300))
- Achieve perfect reproducibility for Linux Bazel builds ([#1543](https://github.com/TraceMachina/nativelink/issues/1543)) - ([4896948](https://github.com/TraceMachina/nativelink/commit/48969489f2d6334a63ff9fb2fe5f4fd082b81d70))
- Implement Local Remote Execution for Rust ([#1510](https://github.com/TraceMachina/nativelink/issues/1510)) - ([5e07ce4](https://github.com/TraceMachina/nativelink/commit/5e07ce4c0a9555edc73c5a1032a164a4a060e2ff))
- Fix `cargo test -p nativelink-store` after 4896b5c ([#1540](https://github.com/TraceMachina/nativelink/issues/1540)) - ([2697eaf](https://github.com/TraceMachina/nativelink/commit/2697eafcaf6675dcebc6c28428f63eb93a622391))
- Decouple automated K8s deployments ([#1531](https://github.com/TraceMachina/nativelink/issues/1531)) - ([a0ca341](https://github.com/TraceMachina/nativelink/commit/a0ca3416ba3e4ed94d6fbdd671ed9a581917fc25))
- Add gnused to createWorker ([#1511](https://github.com/TraceMachina/nativelink/issues/1511)) - ([638c4a7](https://github.com/TraceMachina/nativelink/commit/638c4a7738ad36e39e14b7d53e96078280e19254))
- Fix tests to support nixos pathing ([#1427](https://github.com/TraceMachina/nativelink/issues/1427)) - ([060c128](https://github.com/TraceMachina/nativelink/commit/060c1287b7b6453c8934162b85cccbcb0ccd5a3a))
- Introduce reproducible branch-based coverage ([#1375](https://github.com/TraceMachina/nativelink/issues/1375)) - ([4a51e75](https://github.com/TraceMachina/nativelink/commit/4a51e757a8538da20b626b38ccb7b5ddd73323b8))
- Introduce the NativeLink Cloud flake module ([#1365](https://github.com/TraceMachina/nativelink/issues/1365)) - ([26df13b](https://github.com/TraceMachina/nativelink/commit/26df13b848b52e1bb77e0f98e2fe55e7cdcb81e0))
- Fix broken ca-certificates version in integration tests ([#1367](https://github.com/TraceMachina/nativelink/issues/1367)) - ([ca84219](https://github.com/TraceMachina/nativelink/commit/ca842192883d1e07bae9c6b9fe5877c45bb9eda1))

### ⚙️ Miscellaneous

- Make stores and schedulers lists of named specs ([#1496](https://github.com/TraceMachina/nativelink/issues/1496)) - ([c99dca6](https://github.com/TraceMachina/nativelink/commit/c99dca6d85a23a524102a3e9c7b4cab688fcd6ec))
- Ensure that EvictingMap is threadsafe ([#1564](https://github.com/TraceMachina/nativelink/issues/1564)) - ([4b5fe2e](https://github.com/TraceMachina/nativelink/commit/4b5fe2eef13e4c6322800cc583a13c777c0b4a7b))
- Minor fix to BEP key encoding ([#1539](https://github.com/TraceMachina/nativelink/issues/1539)) - ([c742302](https://github.com/TraceMachina/nativelink/commit/c742302eee9d720d14b0839e684c081fb437182d))
- Move some tools to an externally usable overlay ([#1544](https://github.com/TraceMachina/nativelink/issues/1544)) - ([55a49f3](https://github.com/TraceMachina/nativelink/commit/55a49f30441992ef9feec5c2748f76d5c7ea178c))
- Support native StoreKey in FilesystemStore ([#1489](https://github.com/TraceMachina/nativelink/issues/1489)) - ([679f068](https://github.com/TraceMachina/nativelink/commit/679f068a2e6b27b4e60f242c4e410943181cc068))
- [Experimental] Move identity & origin event middleware config ([#1534](https://github.com/TraceMachina/nativelink/issues/1534)) - ([45520d9](https://github.com/TraceMachina/nativelink/commit/45520d926debe048592011509132069817d6da85))
- Make global lock ConfigMap removable ([#1530](https://github.com/TraceMachina/nativelink/issues/1530)) - ([8782c0b](https://github.com/TraceMachina/nativelink/commit/8782c0bf7e9d55ab7e2bfcf91c4a46bb4ac5f307))
- Move lre-cc into the lre overlay ([#1529](https://github.com/TraceMachina/nativelink/issues/1529)) - ([2c1643d](https://github.com/TraceMachina/nativelink/commit/2c1643d652d788212374fb31f2c2e1f9c3998e28))
- Remove empty top-level GLOSSARY.md ([#1525](https://github.com/TraceMachina/nativelink/issues/1525)) - ([23d5774](https://github.com/TraceMachina/nativelink/commit/23d57743392a593f7fe6a326c35cfd7cd73a042f))
- Rename example configs to json5 ([#1508](https://github.com/TraceMachina/nativelink/issues/1508)) - ([c84f793](https://github.com/TraceMachina/nativelink/commit/c84f793d4423d70c1f8d449e191157e4fdcd2818))
- Discoverable generic blogposts ([#1520](https://github.com/TraceMachina/nativelink/issues/1520)) - ([ad3a501](https://github.com/TraceMachina/nativelink/commit/ad3a501b091e9a7292022fd0a3685a68de088b24))
- adding a semiconductor blog. ([#1518](https://github.com/TraceMachina/nativelink/issues/1518)) - ([d55611a](https://github.com/TraceMachina/nativelink/commit/d55611a292ed47c2c3d06a59659c3361bcfa6b61))
- Migrate rust-overlay patch to an overlay ([#1514](https://github.com/TraceMachina/nativelink/issues/1514)) - ([301e51b](https://github.com/TraceMachina/nativelink/commit/301e51b07a6500f207b4ec1b5f095174fb529bd4))
- Migrate pulumi patches to an overlay ([#1513](https://github.com/TraceMachina/nativelink/issues/1513)) - ([b25fbd1](https://github.com/TraceMachina/nativelink/commit/b25fbd1441acd4ccad68968df270677d8ff7d365))
- Slightly clean up flake ([#1515](https://github.com/TraceMachina/nativelink/issues/1515)) - ([2b18b90](https://github.com/TraceMachina/nativelink/commit/2b18b9001ace5b84e0805d693e7b45360c5e95b2))
- Merge scheduler and cas for K8s ([#1506](https://github.com/TraceMachina/nativelink/issues/1506)) - ([1b7d059](https://github.com/TraceMachina/nativelink/commit/1b7d05933d9376e4aef6c5e93c50d239cdb46034))
- Use an empty instance_name in docker compose example ([#1486](https://github.com/TraceMachina/nativelink/issues/1486)) - ([458527f](https://github.com/TraceMachina/nativelink/commit/458527f84132f8c1bf5c2f67d44a0b2a1d83d235))
- Cleanup some template type definitions ([#1492](https://github.com/TraceMachina/nativelink/issues/1492)) - ([3d04430](https://github.com/TraceMachina/nativelink/commit/3d04430010fa7ecedc45d6c2b41385ceb4b79fb4))
- Bikeshed {Store, Scheduler}Config -> {Store, Scheduler}Spec ([#1483](https://github.com/TraceMachina/nativelink/issues/1483)) - ([7df592f](https://github.com/TraceMachina/nativelink/commit/7df592fd1f195c2ab2de6713799b24f4fde1eb15))
- Make shellexpand fields more robust ([#1471](https://github.com/TraceMachina/nativelink/issues/1471)) - ([b6cf659](https://github.com/TraceMachina/nativelink/commit/b6cf6590211a01125ca662c395eb9dce0a8f7d3d))
- Directly Inject LDFR Script ([#1474](https://github.com/TraceMachina/nativelink/issues/1474)) - ([798e4fe](https://github.com/TraceMachina/nativelink/commit/798e4fe18e1287f30a913c6e2d1fcbef792418e1))
- Stop Redirect Errors ([#1469](https://github.com/TraceMachina/nativelink/issues/1469)) - ([7e766d1](https://github.com/TraceMachina/nativelink/commit/7e766d1800ff57a481d91a00ba9bd84b6bb8c41c))
- Remove case study lacking special approval process ([#1464](https://github.com/TraceMachina/nativelink/issues/1464)) - ([028c91c](https://github.com/TraceMachina/nativelink/commit/028c91c0bcbbc3fd211bdbbb5ac1059bcbdb8455))
- Move custom tekton resources to flux ([#1446](https://github.com/TraceMachina/nativelink/issues/1446)) - ([f877ab0](https://github.com/TraceMachina/nativelink/commit/f877ab09509dcc0461c4ecba7fd9d0ce57ac7c1e))
- Move remaining static content to s3 ([#1444](https://github.com/TraceMachina/nativelink/issues/1444)) - ([8a3869c](https://github.com/TraceMachina/nativelink/commit/8a3869cdddb9202de26bb0ab272519ace73c98f6))
- Really fix LRE/Remote workflow after b44383f ([#1443](https://github.com/TraceMachina/nativelink/issues/1443)) - ([a0e5cf7](https://github.com/TraceMachina/nativelink/commit/a0e5cf7f5b11599674f3167a99068f9c445ce029))
- In redis scheduler removes items that are queued for too long ([#1414](https://github.com/TraceMachina/nativelink/issues/1414)) - ([b68e319](https://github.com/TraceMachina/nativelink/commit/b68e31918945e6a8415ffc7476a871aa290065c1))
- Expose fingerprint hash to metrics in redis store ([#1347](https://github.com/TraceMachina/nativelink/issues/1347)) - ([8a90f09](https://github.com/TraceMachina/nativelink/commit/8a90f097997ea578ee43f4ded449e342455b7daa))
- Redirect indexed broken link ([#1378](https://github.com/TraceMachina/nativelink/issues/1378)) - ([4b4f047](https://github.com/TraceMachina/nativelink/commit/4b4f047798d1ccbc251e96797117baba25ccca4f))
- Enable Nativelink Cloud Cache workflow for macos-14 ([#1374](https://github.com/TraceMachina/nativelink/issues/1374)) - ([6142492](https://github.com/TraceMachina/nativelink/commit/6142492f06e86ba577ef0180a82f176c81f9342b))
- Remove duplicated deno deploy env variables ([#1362](https://github.com/TraceMachina/nativelink/issues/1362)) - ([c17cc34](https://github.com/TraceMachina/nativelink/commit/c17cc34639c3cec31df281c9cc45a9a66aaa2b8f))
- Enable Bazel on darwin ([#1364](https://github.com/TraceMachina/nativelink/issues/1364)) - ([9be5902](https://github.com/TraceMachina/nativelink/commit/9be5902582d1a7cfbe1d20bb7f01e9b85810d848))
- Convert usize to u63 in Store trait APIs ([#1344](https://github.com/TraceMachina/nativelink/issues/1344)) - ([2a55f1e](https://github.com/TraceMachina/nativelink/commit/2a55f1ebd0f0b8c8915af7015f12f59b56593920))
- Remove subscription API from store API ([#1346](https://github.com/TraceMachina/nativelink/issues/1346)) - ([506a297](https://github.com/TraceMachina/nativelink/commit/506a297e84bbb60f93f9f520eb5e09efc5cb500c))
- [Change] BEP Redis key format ([#1345](https://github.com/TraceMachina/nativelink/issues/1345)) - ([ba5b315](https://github.com/TraceMachina/nativelink/commit/ba5b3157a65364ad5e713adb2dc0415987d8f21a))
- ByteStreamServer now responds with no-data-received instead of NotFound ([#1341](https://github.com/TraceMachina/nativelink/issues/1341)) - ([cbb5835](https://github.com/TraceMachina/nativelink/commit/cbb5835df40f4f75aacfb586b5e64d8b4e166aaa))
- DigestInfo now does string conversions on the stack ([#1338](https://github.com/TraceMachina/nativelink/issues/1338)) - ([a68392a](https://github.com/TraceMachina/nativelink/commit/a68392a0b911b806cd9a1cd8154789b72ce3ddc8))
- Delete ~/Applications and iOS simulators/cache from Mac runners ([#1334](https://github.com/TraceMachina/nativelink/issues/1334)) - ([f533d30](https://github.com/TraceMachina/nativelink/commit/f533d3023c7e604b849ca4882aa2a276c7fe2dbd))
- Cleanup digest function to use u64 instead of i64 ([#1327](https://github.com/TraceMachina/nativelink/issues/1327)) - ([140b7cb](https://github.com/TraceMachina/nativelink/commit/140b7cba8c21ba9f6f92ffaa342cc07c64b0b188))
- Improve docker image for RBE and re-enable RBE on main ([#1326](https://github.com/TraceMachina/nativelink/issues/1326)) - ([84eab85](https://github.com/TraceMachina/nativelink/commit/84eab85ac7c1e98506e9fdf0749f38db65d057c4))
- Improve debugging on some error messages ([#1313](https://github.com/TraceMachina/nativelink/issues/1313)) - ([514da4b](https://github.com/TraceMachina/nativelink/commit/514da4b6c108b28d7ac1467290a8286d22dbd8e4))
- Change AwaitedAction's API to always return Result<Stream> ([#1312](https://github.com/TraceMachina/nativelink/issues/1312)) - ([dea9d18](https://github.com/TraceMachina/nativelink/commit/dea9d187270783c93c4b63c9099a254d9bede8a4))
- AwaitedAction's operation_id and client_operation_id now separated ([#1311](https://github.com/TraceMachina/nativelink/issues/1311)) - ([00fa82d](https://github.com/TraceMachina/nativelink/commit/00fa82d08ef2a79c482cdea62aa33e9df9b8bb9b))
- SimpleScheduler version matching uses Aborted to know if failure ([#1308](https://github.com/TraceMachina/nativelink/issues/1308)) - ([753c1e7](https://github.com/TraceMachina/nativelink/commit/753c1e7369be7c3f18b6f3da442242fe55bcf6fa))
- Prepare scheduler config & move owner of notify task change owner ([#1306](https://github.com/TraceMachina/nativelink/issues/1306)) - ([17acce2](https://github.com/TraceMachina/nativelink/commit/17acce2546b721d9506d19becd5e08e12c6c13c3))
- Pass deno deploy token ([#1321](https://github.com/TraceMachina/nativelink/issues/1321)) - ([057d91d](https://github.com/TraceMachina/nativelink/commit/057d91d6b3da61f418e0830fda1ef911ff9f3f4a))
- Move where increment_version() is triggered for scheduler code ([#1307](https://github.com/TraceMachina/nativelink/issues/1307)) - ([7736a6f](https://github.com/TraceMachina/nativelink/commit/7736a6f0e53123cfe7637c2000ad9b2ff5dc2478))
- Move ClientActionStateResult to SimpleSchedulerStateManager ([#1305](https://github.com/TraceMachina/nativelink/issues/1305)) - ([4b45662](https://github.com/TraceMachina/nativelink/commit/4b45662ae4e07e13ee851040ec00c754b15ac34f))

### ⬆️ Bumps & Version Updates

- Update Rust crate serde_json to v1.0.138 ([#1560](https://github.com/TraceMachina/nativelink/issues/1560)) - ([a67d4bd](https://github.com/TraceMachina/nativelink/commit/a67d4bd2eba9132850aa5b5eeb86cbe209eeeb82))
- Bump deps ([#1559](https://github.com/TraceMachina/nativelink/issues/1559)) - ([4772bd4](https://github.com/TraceMachina/nativelink/commit/4772bd4d0f69c4a8e94f65a7e960c2f44ba63dca))
- Bump Rust deps ([#1536](https://github.com/TraceMachina/nativelink/issues/1536)) - ([4896b5c](https://github.com/TraceMachina/nativelink/commit/4896b5c70f6c986b2565a7777b1c37c1c1054be0))
- Bump Go deps ([#1535](https://github.com/TraceMachina/nativelink/issues/1535)) - ([61f1df7](https://github.com/TraceMachina/nativelink/commit/61f1df7dea0e4b27742d4b7cea50710177e5e3ad))
- Update company site on web/platform ([#1521](https://github.com/TraceMachina/nativelink/issues/1521)) - ([8671931](https://github.com/TraceMachina/nativelink/commit/8671931634dc7e8506e23b5014b05b7733399e47))
- Update terms on web/platform ([#1517](https://github.com/TraceMachina/nativelink/issues/1517)) - ([5804568](https://github.com/TraceMachina/nativelink/commit/5804568c2e14f3f70271a00e96dca70476cb65d8))
- Bump rust deps ([#1499](https://github.com/TraceMachina/nativelink/issues/1499)) - ([c458871](https://github.com/TraceMachina/nativelink/commit/c458871a8e0678645b2f6714a9eb83c8e748c62e))
- Bump go deps ([#1495](https://github.com/TraceMachina/nativelink/issues/1495)) - ([afe0f4c](https://github.com/TraceMachina/nativelink/commit/afe0f4c02ef6bd3586e87a4c3d396be9ff7aa0e8))
- Bump nightly rust to 2024-11-23 ([#1494](https://github.com/TraceMachina/nativelink/issues/1494)) - ([decdc7f](https://github.com/TraceMachina/nativelink/commit/decdc7feb3436aa459a021e6fff829972d3833be))
- Bump flake ([#1493](https://github.com/TraceMachina/nativelink/issues/1493)) - ([99b9cbb](https://github.com/TraceMachina/nativelink/commit/99b9cbbf4e2bdb854b7ddc2cd7b7889838c3de31))
- Update Partytown ([#1467](https://github.com/TraceMachina/nativelink/issues/1467)) - ([3fbc273](https://github.com/TraceMachina/nativelink/commit/3fbc273110f5d7f72966ee8e8abc2dc1296eec71))
- Update company site on web platform ([#1451](https://github.com/TraceMachina/nativelink/issues/1451)) - ([cb5d0bc](https://github.com/TraceMachina/nativelink/commit/cb5d0bc82fab709010b2eb8b442eef01fa259301))
- Update company site on web platform ([#1429](https://github.com/TraceMachina/nativelink/issues/1429)) - ([e68da64](https://github.com/TraceMachina/nativelink/commit/e68da648ad6a2e5e3b8f1e3e7e1e5dae58bbc27e))
- Bump nontrivial Rust dependencies ([#1402](https://github.com/TraceMachina/nativelink/issues/1402)) - ([f541cbb](https://github.com/TraceMachina/nativelink/commit/f541cbbf630cb5dd54105835bc3bb738bb8b428f))
- Update rust dependencies ([#1381](https://github.com/TraceMachina/nativelink/issues/1381)) - ([b5a4d92](https://github.com/TraceMachina/nativelink/commit/b5a4d928a817a7bdf7466cf01253fb1d92ee880f))
- Update web workflow ([#1370](https://github.com/TraceMachina/nativelink/issues/1370)) - ([68753c6](https://github.com/TraceMachina/nativelink/commit/68753c663159100d7ae66bef50d00e12337c9066))
- Bump toolchains ([#1356](https://github.com/TraceMachina/nativelink/issues/1356)) - ([4d331f7](https://github.com/TraceMachina/nativelink/commit/4d331f7332f8835bf57bd75ebd0c7e09635119db))
- Update web dependencies ([#1354](https://github.com/TraceMachina/nativelink/issues/1354)) - ([f31015d](https://github.com/TraceMachina/nativelink/commit/f31015d96f47aef6daf63e405364c38679f29df6))
- Bump the scorecard action ([#1330](https://github.com/TraceMachina/nativelink/issues/1330)) - ([57c784a](https://github.com/TraceMachina/nativelink/commit/57c784ac3d444d86ab501b14ab8662856bbeb4c7))

## [0.5.3](https://github.com/TraceMachina/nativelink/compare/v0.5.1..v0.5.3) - 2024-09-04



### ⛰️  Features

- Add more metrics & event messages ([#1303](https://github.com/TraceMachina/nativelink/issues/1303)) - ([9f0e809](https://github.com/TraceMachina/nativelink/commit/9f0e8093a7fae116153e8e8e988d55d45e9a7836))

### 🐛 Bug Fixes

- Fix bug in redis store when zero data stored but data does not exist ([#1304](https://github.com/TraceMachina/nativelink/issues/1304)) - ([59020f1](https://github.com/TraceMachina/nativelink/commit/59020f1e9c7f103afc4a8246dc17cae9910b3121))
- Fix bug where OperationId::String was being used instead of Uuid version ([#1301](https://github.com/TraceMachina/nativelink/issues/1301)) - ([cc611cd](https://github.com/TraceMachina/nativelink/commit/cc611cd665edc7c99113d8f47c1a27be46e04843))
- Fix rare case where eof was sent on buf_channel when retry happens ([#1295](https://github.com/TraceMachina/nativelink/issues/1295)) - ([47dfc20](https://github.com/TraceMachina/nativelink/commit/47dfc209aaa16f15e9e45fab41e5e5682b8d6639))
- Fix Tekton depedency order within Pulumi ([#1291](https://github.com/TraceMachina/nativelink/issues/1291)) - ([0fd0a94](https://github.com/TraceMachina/nativelink/commit/0fd0a94c808e23f73c80e7f119d0cc6f6a829e07))
- Revert "Release NativeLink v0.5.2 ([#1283](https://github.com/TraceMachina/nativelink/issues/1283))" ([#1284](https://github.com/TraceMachina/nativelink/issues/1284)) - ([1b38a64](https://github.com/TraceMachina/nativelink/commit/1b38a64cad4b9b9e099cfeaca6b7394685458377))
- Fix verify_size w/ verify_hash set to true in VerifyStore ([#1273](https://github.com/TraceMachina/nativelink/issues/1273)) - ([c21d59f](https://github.com/TraceMachina/nativelink/commit/c21d59f104cb7910e05e2633693d2c5203c6fb74))

### 📚 Documentation

- Re-enable docs auto-deployment on main ([#1317](https://github.com/TraceMachina/nativelink/issues/1317)) - ([ca88d90](https://github.com/TraceMachina/nativelink/commit/ca88d90d2ad517344bd7b42e871625d4bdbcc6ca))
- Migrate docs buildsystem from pnpm to bun ([#1268](https://github.com/TraceMachina/nativelink/issues/1268)) - ([ef3a8a6](https://github.com/TraceMachina/nativelink/commit/ef3a8a6bb3605ed9433d712f7b8449907db73a85))
- Fix `docs` build warning from `nativelink-config` ([#1270](https://github.com/TraceMachina/nativelink/issues/1270)) - ([5903a8e](https://github.com/TraceMachina/nativelink/commit/5903a8e82ce4f441882a41e8a8d12ba6e47b1ca0))
- Fix invalid links in the documentation ([#1256](https://github.com/TraceMachina/nativelink/issues/1256)) - ([ae0c82c](https://github.com/TraceMachina/nativelink/commit/ae0c82c06fff8753c083ee8d5e791d9807ec7498))
- Add 90s Explainer to README.md ([#1254](https://github.com/TraceMachina/nativelink/issues/1254)) - ([a3cf01c](https://github.com/TraceMachina/nativelink/commit/a3cf01c5f094571fcd370f9dfde9a4de648cb11b))
- Explicitly map hostport in README ([#1255](https://github.com/TraceMachina/nativelink/issues/1255)) - ([7777938](https://github.com/TraceMachina/nativelink/commit/7777938294047377cb4ce9f4d8649c45055596ed))

### 🧪 Testing & CI

- Fix nix2container skopeo patch hash ([#1294](https://github.com/TraceMachina/nativelink/issues/1294)) - ([689d099](https://github.com/TraceMachina/nativelink/commit/689d099460fb9ce07e27b16bc02c117a13604c66))
- Fix broken variables in NativeLink Cloud CI jobs and disable RBE test ([#1293](https://github.com/TraceMachina/nativelink/issues/1293)) - ([f4ae4cc](https://github.com/TraceMachina/nativelink/commit/f4ae4ccd09c1b4d00b3212c39e0cfbe71ce2e53d))
- Fix typos in code comments ([#1190](https://github.com/TraceMachina/nativelink/issues/1190)) - ([3e1fcbd](https://github.com/TraceMachina/nativelink/commit/3e1fcbdefc55a71e7574dca90e1ab3aa7d6951a3))

### ⚙️ Miscellaneous

- S3 store will now retry more aggresively ([#1302](https://github.com/TraceMachina/nativelink/issues/1302)) - ([0ecf5b4](https://github.com/TraceMachina/nativelink/commit/0ecf5b43d8046a119cf236c972b55208df3c6520))
- Remove nix2container patch hash workaround ([#1296](https://github.com/TraceMachina/nativelink/issues/1296)) - ([d5c55ac](https://github.com/TraceMachina/nativelink/commit/d5c55ac16cfe4ee56aed6baa6923617db4236242))
- Use docker to create a buck2 image ([#1275](https://github.com/TraceMachina/nativelink/issues/1275)) - ([8896b65](https://github.com/TraceMachina/nativelink/commit/8896b65fed8feeb76b2f3d62711a03f40acb4b22))
- Support remote build execution on main and read-only remote cache on PRs ([#1277](https://github.com/TraceMachina/nativelink/issues/1277)) - ([2f9fd8b](https://github.com/TraceMachina/nativelink/commit/2f9fd8b199adb3a4482930afa27982f0c70bdcce))
- Revert "Make de/serialized structs compliant with Rust naming practices ([#1271](https://github.com/TraceMachina/nativelink/issues/1271))" ([#1282](https://github.com/TraceMachina/nativelink/issues/1282)) - ([0933c1a](https://github.com/TraceMachina/nativelink/commit/0933c1ad4e531565f34e281b55e1d4d007c53eae))
- Make de/serialized structs compliant with Rust naming practices ([#1271](https://github.com/TraceMachina/nativelink/issues/1271)) - ([a174fbf](https://github.com/TraceMachina/nativelink/commit/a174fbfbd9082110146a4ca497739084ea367892))
- Append buck2 toolchain with additional packages ([#1264](https://github.com/TraceMachina/nativelink/issues/1264)) - ([042f4a5](https://github.com/TraceMachina/nativelink/commit/042f4a5d25abe6efebde2f7dd7b2bb450d25b6f1))
- Remove ActionScheduler and introduce KnownPlatformPropertyProvider ([#1260](https://github.com/TraceMachina/nativelink/issues/1260)) - ([9c87370](https://github.com/TraceMachina/nativelink/commit/9c873706cb8f7e43ae70c791108ae1a9e9939d2b))
- add static size and fix meta-typo ([#1261](https://github.com/TraceMachina/nativelink/issues/1261)) - ([bddee33](https://github.com/TraceMachina/nativelink/commit/bddee33446456cf68d88e8f192821721baf856b8))
- Raise correct error if BEP service fails ([#1259](https://github.com/TraceMachina/nativelink/issues/1259)) - ([6b7401a](https://github.com/TraceMachina/nativelink/commit/6b7401afdf9ae093c6223d1dea711e7b8b1c940a))
- Crosscompile NativeLink ([#1233](https://github.com/TraceMachina/nativelink/issues/1233)) - ([ab64efd](https://github.com/TraceMachina/nativelink/commit/ab64efdfaab6e312dd13e27ab56f7871ced31b93))

### ⬆️ Bumps & Version Updates

- Bump Rust dependencies ([#1319](https://github.com/TraceMachina/nativelink/issues/1319)) - ([34db1b8](https://github.com/TraceMachina/nativelink/commit/34db1b8cad112531bbba3b0bdef56c1d3ccc577f))
- Update Rust crate clap to v4.5.15 ([#1225](https://github.com/TraceMachina/nativelink/issues/1225)) - ([4bc246a](https://github.com/TraceMachina/nativelink/commit/4bc246a23f02d2838e5d700dde2e30e8f07ab407))

## [0.5.1](https://github.com/TraceMachina/nativelink/compare/v0.5.0..v0.5.1) - 2024-08-08



### 🐛 Bug Fixes

- [Bug] Add rt-tokio feature to aws-sdk-s3 ([#1248](https://github.com/TraceMachina/nativelink/issues/1248)) - ([3eadab0](https://github.com/TraceMachina/nativelink/commit/3eadab01d23177deb207d148bb2ab883f2f66a4f))

### ⚙️ Miscellaneous

- Conversion implementations for awaited action db structs ([#1243](https://github.com/TraceMachina/nativelink/issues/1243)) - ([d5f2781](https://github.com/TraceMachina/nativelink/commit/d5f2781eff92432ceea9497f7b1fe1c3b672eda4))
- Make redis clients available on RedisStore ([#1244](https://github.com/TraceMachina/nativelink/issues/1244)) - ([c3f648e](https://github.com/TraceMachina/nativelink/commit/c3f648ecaad4861983bce1a5dc67781685bd1e80))

## [0.5.0](https://github.com/TraceMachina/nativelink/compare/v0.4.0..v0.5.0) - 2024-08-07



### ❌️  Breaking Changes

- [Breaking] Digest function now auto-detected from request ([#899](https://github.com/TraceMachina/nativelink/issues/899)) - ([0a33c83](https://github.com/TraceMachina/nativelink/commit/0a33c8399e38e9aeb1d76c41f0663d16e9f938ec))

### ⛰️  Features

- Add example clang/rust/go toolchain ([#1200](https://github.com/TraceMachina/nativelink/issues/1200)) - ([11298d8](https://github.com/TraceMachina/nativelink/commit/11298d831929950db0af9d9df7c64ddeeb5f35b6))
- Introduce NL_LOG to control logging format ([#1154](https://github.com/TraceMachina/nativelink/issues/1154)) - ([d9922b3](https://github.com/TraceMachina/nativelink/commit/d9922b370ab680602e7669a1480b6fa6694aaa1e))
- Add Capacitor dashboard to devcluster ([#1115](https://github.com/TraceMachina/nativelink/issues/1115)) - ([93ae95a](https://github.com/TraceMachina/nativelink/commit/93ae95aa6dc43fe368071bcdf47ab147863328bc))
- Add Flux to development cluster ([#1096](https://github.com/TraceMachina/nativelink/issues/1096)) - ([6a40374](https://github.com/TraceMachina/nativelink/commit/6a403743eb14e114be760cd6ee1f5157f3b16f82))
- Allow Tekton pipelines to be triggered by Flux Alerts ([#1094](https://github.com/TraceMachina/nativelink/issues/1094)) - ([5de75cc](https://github.com/TraceMachina/nativelink/commit/5de75ccc5059a49f9ca0a72135bb914146f47ddf))
- Allow WebSocket upgrades in devcluster Loadbalancer ([#1098](https://github.com/TraceMachina/nativelink/issues/1098)) - ([dda8c31](https://github.com/TraceMachina/nativelink/commit/dda8c31a8ebb0ce104b1850dc2c07a398edb48e3))
- Implement RedisStateManager ([#1023](https://github.com/TraceMachina/nativelink/issues/1023)) - ([5104778](https://github.com/TraceMachina/nativelink/commit/510477867454140f605663f8accf4461272978fe))
- Add optional and experimental pub sub publisher for redis store write. ([#1027](https://github.com/TraceMachina/nativelink/issues/1027)) - ([128ba2a](https://github.com/TraceMachina/nativelink/commit/128ba2a6c02c6c16d6d1b82d3f731063bc5b7117))
- Decouple nativelink from toolchain containers ([#1013](https://github.com/TraceMachina/nativelink/issues/1013)) - ([00e5bb3](https://github.com/TraceMachina/nativelink/commit/00e5bb3406505bff561ef3c53db2d69d621b7559))
- Add Bazel rules for generating rust-project.json ([#1019](https://github.com/TraceMachina/nativelink/issues/1019)) - ([bb91fa9](https://github.com/TraceMachina/nativelink/commit/bb91fa990d56e57eb7fcb31543e333cd1a558435))
- Add list api to StoreApi and MemoryStore ([#1003](https://github.com/TraceMachina/nativelink/issues/1003)) - ([5a78919](https://github.com/TraceMachina/nativelink/commit/5a78919ad5c261aae50aa379fbb6aa44e4bf0536))
- Add memory store optimized subscription API ([#988](https://github.com/TraceMachina/nativelink/issues/988)) - ([bf9edc9](https://github.com/TraceMachina/nativelink/commit/bf9edc9c0a034cfedaa51f039123cb29278d3f7e))
- Add serialize and deserialize to structs ([#965](https://github.com/TraceMachina/nativelink/issues/965)) - ([79908cb](https://github.com/TraceMachina/nativelink/commit/79908cb17684fb23bd482e340bb5685f95b92d4b))
- Add subscribe API to Store API ([#924](https://github.com/TraceMachina/nativelink/issues/924)) - ([3be7255](https://github.com/TraceMachina/nativelink/commit/3be725561b071a639b276a0c3e1771940c6a23ac))
- Add a config option to prefix keys in Redis stores ([#981](https://github.com/TraceMachina/nativelink/issues/981)) - ([b7a7e36](https://github.com/TraceMachina/nativelink/commit/b7a7e364e78b07a907407856354a61c54e12406f))
- Add OrderBy field for OperationFilter ([#969](https://github.com/TraceMachina/nativelink/issues/969)) - ([a911af4](https://github.com/TraceMachina/nativelink/commit/a911af48f84e05e85e040c6733de38b02c783308))
- Add initial support for BEP (Build Event Protocol) ([#961](https://github.com/TraceMachina/nativelink/issues/961)) - ([23cba13](https://github.com/TraceMachina/nativelink/commit/23cba13f9bb1a51360d8cc7818ea4320f1ac40cd))
- Convert RedisError into nativelink Error ([#959](https://github.com/TraceMachina/nativelink/issues/959)) - ([cabc0c3](https://github.com/TraceMachina/nativelink/commit/cabc0c326bdd6c2a65eedff5f87cb56f2f1d322e))
- Add JSON config examples to store.rs ([#967](https://github.com/TraceMachina/nativelink/issues/967)) - ([da9399b](https://github.com/TraceMachina/nativelink/commit/da9399b7a94f3d40f16e42488123dfa97031f6b9))
- Make quantity field human readable ([#891](https://github.com/TraceMachina/nativelink/issues/891)) - ([da2c4a7](https://github.com/TraceMachina/nativelink/commit/da2c4a70662267b2f8e8992ea42a439a0e7ab2ec))
- Add drake toolchain configs ([#942](https://github.com/TraceMachina/nativelink/issues/942)) - ([e65c04a](https://github.com/TraceMachina/nativelink/commit/e65c04a3ab8b14677e11778e2c3d2fc4bc501bc0))
- Add Operation State Manager API ([#937](https://github.com/TraceMachina/nativelink/issues/937)) - ([1d2d838](https://github.com/TraceMachina/nativelink/commit/1d2d838e40065b4f4b0eb3a27f0fa2a6c7cecf2f))

### 🐛 Bug Fixes

- Fix docker-compose ([#1238](https://github.com/TraceMachina/nativelink/issues/1238)) - ([44bc795](https://github.com/TraceMachina/nativelink/commit/44bc795955f7cdcdded46e72cdb2b7779bec359c))
- Fix compile time warnings from rustc version upgrade ([#1231](https://github.com/TraceMachina/nativelink/issues/1231)) - ([7f9f2da](https://github.com/TraceMachina/nativelink/commit/7f9f2da707c1cb9199b2f43fa789cbe87cabea2a))
- Fix S3 store missing not having sleep function ([#1220](https://github.com/TraceMachina/nativelink/issues/1220)) - ([827a000](https://github.com/TraceMachina/nativelink/commit/827a0002c49794904fac07e24a8a382bf9691e1e))
- Fix case when scheduler drops action on client reconnect ([#1198](https://github.com/TraceMachina/nativelink/issues/1198)) - ([0b40639](https://github.com/TraceMachina/nativelink/commit/0b406393a6f39d306ce6ff287d753e86a6a7069a))
- Fix bad practice bazelrc naming scheme ([#1183](https://github.com/TraceMachina/nativelink/issues/1183)) - ([8d843e8](https://github.com/TraceMachina/nativelink/commit/8d843e8806a420599c1b3561a9870038e8da0ca2))
- Fix bug in S3 where it ignores EOF ([#1178](https://github.com/TraceMachina/nativelink/issues/1178)) - ([f3e58a2](https://github.com/TraceMachina/nativelink/commit/f3e58a24d9a974e044da2c6e23278019fba4223c))
- Fix clippy::manual_string_new ([#1106](https://github.com/TraceMachina/nativelink/issues/1106)) - ([3992aef](https://github.com/TraceMachina/nativelink/commit/3992aefd939b0a65464b9a87c484cf57de5672f5))
- Fix script bugs ([#1147](https://github.com/TraceMachina/nativelink/issues/1147)) - ([2e85c90](https://github.com/TraceMachina/nativelink/commit/2e85c9078d0eb9046a26df009aa022bff9039153))
- Fix chromium demo ([#1144](https://github.com/TraceMachina/nativelink/issues/1144)) - ([00a7134](https://github.com/TraceMachina/nativelink/commit/00a71341630701e8fffe21bf563b201810c50f13))
- Fix filesystem_cas.json ([#1111](https://github.com/TraceMachina/nativelink/issues/1111)) - ([0cbddba](https://github.com/TraceMachina/nativelink/commit/0cbddba39ac192cb3a0106a0755f0b5a2d70c569))
- Fix vale issues in MDX files ([#1086](https://github.com/TraceMachina/nativelink/issues/1086)) - ([a3bd7d9](https://github.com/TraceMachina/nativelink/commit/a3bd7d95ad33ac60cbed849582dc16c4d59bb7fa))
- Unbreak LRE Remote workflow ([#1058](https://github.com/TraceMachina/nativelink/issues/1058)) - ([2adda24](https://github.com/TraceMachina/nativelink/commit/2adda2475eed578d610a66b98f965922656061af))
- Fix Cargo mismatch on MacOS build ([#974](https://github.com/TraceMachina/nativelink/issues/974)) - ([591126d](https://github.com/TraceMachina/nativelink/commit/591126d6531f36a5365cbedfe1c6f165a14b0ab6))
- Explicitly set deleted timestamp in trivy ([#1006](https://github.com/TraceMachina/nativelink/issues/1006)) - ([43f1aeb](https://github.com/TraceMachina/nativelink/commit/43f1aeb18c5cdc26c3de516e7448a0c44489b9e9))
- Register metrics on PropertyModifierScheduler ([#954](https://github.com/TraceMachina/nativelink/issues/954)) - ([b1d6c40](https://github.com/TraceMachina/nativelink/commit/b1d6c406b1d8d12ec4d06d8d179b4b1f97d75f90))
- Unbreak docker-compose workflow ([#940](https://github.com/TraceMachina/nativelink/issues/940)) - ([fce476f](https://github.com/TraceMachina/nativelink/commit/fce476f70c3ec6f06c5399bbfaf322677a0b9b32))

### 📚 Documentation

- Update README.md ([#1232](https://github.com/TraceMachina/nativelink/issues/1232)) - ([7b5231f](https://github.com/TraceMachina/nativelink/commit/7b5231ffd99f60fdfce8592912719b31ffa50c72))
- Add CI focused content to api key docs ([#1196](https://github.com/TraceMachina/nativelink/issues/1196)) - ([5798761](https://github.com/TraceMachina/nativelink/commit/57987612547fa151a54a4b196671c0dcc3c15c5f))
- Add read only key instructions to api key docs ([#1187](https://github.com/TraceMachina/nativelink/issues/1187)) - ([d37bd90](https://github.com/TraceMachina/nativelink/commit/d37bd90a314890fe901235e0432d263faa66d221))
- Add new API key prod docs ([#1185](https://github.com/TraceMachina/nativelink/issues/1185)) - ([f59f8ba](https://github.com/TraceMachina/nativelink/commit/f59f8ba69eacd21715b1b210cbb06220ea31cbb3))
- Fix typos in the documentation and comments ([#1174](https://github.com/TraceMachina/nativelink/issues/1174)) - ([9948737](https://github.com/TraceMachina/nativelink/commit/9948737fbbfd7b36e126ad5ab64f9f6936de96dd))
- Polish cloud docs for Bazel and Pants ([#1152](https://github.com/TraceMachina/nativelink/issues/1152)) - ([c54fe00](https://github.com/TraceMachina/nativelink/commit/c54fe00c500e9fbced8cb85fe77e931818a67eb1))
- Fix an accessibility issue in the README ([#1149](https://github.com/TraceMachina/nativelink/issues/1149)) - ([53215a9](https://github.com/TraceMachina/nativelink/commit/53215a91cfb780dd8f5dd0aae81411009476c67c))
- Overhaul NativeLink Documentation ([#1138](https://github.com/TraceMachina/nativelink/issues/1138)) - ([71dee56](https://github.com/TraceMachina/nativelink/commit/71dee569d14d773a9470dc79f5cf64f775c51a2b))
- Disable some workflows on PRs that only change docs ([#1148](https://github.com/TraceMachina/nativelink/issues/1148)) - ([506c144](https://github.com/TraceMachina/nativelink/commit/506c144b30c4521278eea0d51542c3d023b036fb))
- Fix overflowing mermaid diagrams in docs ([#1133](https://github.com/TraceMachina/nativelink/issues/1133)) - ([5810489](https://github.com/TraceMachina/nativelink/commit/5810489465ae9ae879c181026487d703b1d370e5))
- Update README.md ([#1134](https://github.com/TraceMachina/nativelink/issues/1134)) - ([ff90c34](https://github.com/TraceMachina/nativelink/commit/ff90c340416a8c96b4e54cda3ac51dd0d6426f1c))
- Fix README after 612b86e ([#1132](https://github.com/TraceMachina/nativelink/issues/1132)) - ([e93b869](https://github.com/TraceMachina/nativelink/commit/e93b869b78011ab1acf9524a8469f354e2e91f2d))
- Move installation instructions to new docs ([#1127](https://github.com/TraceMachina/nativelink/issues/1127)) - ([612b86e](https://github.com/TraceMachina/nativelink/commit/612b86e6565298b7c1ee6846dc9b8790d1e4dd1b))
- fixed the docs and removed errant TODO. ([#1085](https://github.com/TraceMachina/nativelink/issues/1085)) - ([f777126](https://github.com/TraceMachina/nativelink/commit/f777126f109bfc652ff085d3658d42c079f11999))
- Improve README branding and links ([#1083](https://github.com/TraceMachina/nativelink/issues/1083)) - ([eb8fc9f](https://github.com/TraceMachina/nativelink/commit/eb8fc9f58d789e37dde33a7cab8ee8137c22d3fb))
- Revert "Improve README branding and links ([#1074](https://github.com/TraceMachina/nativelink/issues/1074))" ([#1080](https://github.com/TraceMachina/nativelink/issues/1080)) - ([2bdd9bd](https://github.com/TraceMachina/nativelink/commit/2bdd9bdc5660a17d5315cfcf8527892275dcf2fb))
- Improve README branding and links ([#1074](https://github.com/TraceMachina/nativelink/issues/1074)) - ([1f107e4](https://github.com/TraceMachina/nativelink/commit/1f107e4666a8bc046ea5356008450f7d83ef77a8))
- Reorder `README` ([#1077](https://github.com/TraceMachina/nativelink/issues/1077)) - ([aedf2ef](https://github.com/TraceMachina/nativelink/commit/aedf2ef28d98bc31ccec33061a56f53522c9e205))
- Reimplement documentation infrastructure ([#1056](https://github.com/TraceMachina/nativelink/issues/1056)) - ([67e3164](https://github.com/TraceMachina/nativelink/commit/67e31640cd8bf3232763c0e7d298b54a35fc32ac))
- Move Terraform examples to graveyard ([#1016](https://github.com/TraceMachina/nativelink/issues/1016)) - ([af4c1de](https://github.com/TraceMachina/nativelink/commit/af4c1de47d6f98b942688a0f5278c815cde306df))
- Introduce basic rustdoc infrastructure ([#980](https://github.com/TraceMachina/nativelink/issues/980)) - ([af87ec1](https://github.com/TraceMachina/nativelink/commit/af87ec151345ddc79f9fcf669199e04b9bbdd606))
- Expand configuration documentation ([#970](https://github.com/TraceMachina/nativelink/issues/970)) - ([c0c09ed](https://github.com/TraceMachina/nativelink/commit/c0c09ed3de52573385d783868156824bafcce09d))
- Update images for docs ([#930](https://github.com/TraceMachina/nativelink/issues/930)) - ([b7b58a7](https://github.com/TraceMachina/nativelink/commit/b7b58a7af3378d14780970f39e918e9d64131777))
- Update old tag version in `README.md` ([#923](https://github.com/TraceMachina/nativelink/issues/923)) - ([ec257fe](https://github.com/TraceMachina/nativelink/commit/ec257fe2814574611c2004599e6033c636e9e8c1))

### 🧪 Testing & CI

- Remove some needless CI tests ([#1240](https://github.com/TraceMachina/nativelink/issues/1240)) - ([3e259fd](https://github.com/TraceMachina/nativelink/commit/3e259fd9eb28fd6b246e256ec9b21133cd5239c1))
- Fix Cargo.toml files when using cargo test on specific packages ([#1236](https://github.com/TraceMachina/nativelink/issues/1236)) - ([ba7abf3](https://github.com/TraceMachina/nativelink/commit/ba7abf395a63a13ae46e23aaf4a6e50a5f52f3b9))
- Remove nativelink-proto as build dependency ([#1209](https://github.com/TraceMachina/nativelink/issues/1209)) - ([19f4483](https://github.com/TraceMachina/nativelink/commit/19f4483979384a62f142ed35927a6919df057940))
- Significantly reduce Bazel test time ([#1210](https://github.com/TraceMachina/nativelink/issues/1210)) - ([4f49d53](https://github.com/TraceMachina/nativelink/commit/4f49d53b371e2f2069c726fc89766b6fa3c1ce18))
- [Refactor] Overhaul of scheduler component ([#1169](https://github.com/TraceMachina/nativelink/issues/1169)) - ([3b8c3a5](https://github.com/TraceMachina/nativelink/commit/3b8c3a583b7df12bddba188fe2df221523c6b0f5))
- Add BEP to CI ([#1124](https://github.com/TraceMachina/nativelink/issues/1124)) - ([fa7b099](https://github.com/TraceMachina/nativelink/commit/fa7b099ba73e408bc02c9b99b22c1dcb65a269be))
- Fix bystream_server_tests ([#1087](https://github.com/TraceMachina/nativelink/issues/1087)) - ([846b25b](https://github.com/TraceMachina/nativelink/commit/846b25bc0c236d0abdf63b63dc11873993ef9894))
- Reduce references to self.state_manager.inner ([#1060](https://github.com/TraceMachina/nativelink/issues/1060)) - ([2eefa75](https://github.com/TraceMachina/nativelink/commit/2eefa75afe702c0fe6d1e5761bd5cc32c74bbba4))
- Fixes cyclical dependency between util and store ([#1017](https://github.com/TraceMachina/nativelink/issues/1017)) - ([200f976](https://github.com/TraceMachina/nativelink/commit/200f97699df10133488c32bc765154db69c1238c))
- [bug] Ensure OperationId is used at external protocol points ([#1001](https://github.com/TraceMachina/nativelink/issues/1001)) - ([5ffaf89](https://github.com/TraceMachina/nativelink/commit/5ffaf89bc90ae4bd2154f8b8615afe83d3338b50))
- Remove installation test from devShell ([#1014](https://github.com/TraceMachina/nativelink/issues/1014)) - ([9c40d57](https://github.com/TraceMachina/nativelink/commit/9c40d579f9f4c5800aefc0c3996ddea6c0a112f7))
- Increase timeout of pre-commit-checks CI pipeline ([#1009](https://github.com/TraceMachina/nativelink/issues/1009)) - ([2d64361](https://github.com/TraceMachina/nativelink/commit/2d6436158760c0a869cde8c1417e990221e83bf3))
- Add CI test to run on nativelink.com ([#1007](https://github.com/TraceMachina/nativelink/issues/1007)) - ([3bc14bd](https://github.com/TraceMachina/nativelink/commit/3bc14bd53900f50774b4bac6ffce5c4da8d657b9))
- Create scheduler state module ([#968](https://github.com/TraceMachina/nativelink/issues/968)) - ([264edb7](https://github.com/TraceMachina/nativelink/commit/264edb7ffbdf7e73850bd0a066f0e3a9b87b4bf3))
- Remove extraneous mod statements from tests ([#975](https://github.com/TraceMachina/nativelink/issues/975)) - ([f59a1d7](https://github.com/TraceMachina/nativelink/commit/f59a1d72b45546d6f7ec72e6b0d72bcfbfaab221))
- Add dev build profile and remove lto from CI ([#976](https://github.com/TraceMachina/nativelink/issues/976)) - ([cec25fb](https://github.com/TraceMachina/nativelink/commit/cec25fb0fe312b87768c525439316fa20d6083cf))
- Fix pulumi ratelimiting build error ([#953](https://github.com/TraceMachina/nativelink/issues/953)) - ([03841cc](https://github.com/TraceMachina/nativelink/commit/03841cc340816058363d7a2958d0dbc31113c1de))
- Add kind-loadbalancer ([#929](https://github.com/TraceMachina/nativelink/issues/929)) - ([c42fd0d](https://github.com/TraceMachina/nativelink/commit/c42fd0d9f93b5f41f2df6d23d529ce40d1568c55))

### ⚙️ Miscellaneous

- Migrate much of the ActionScheduler API to ClientStateManager API ([#1241](https://github.com/TraceMachina/nativelink/issues/1241)) - ([2b8f1ee](https://github.com/TraceMachina/nativelink/commit/2b8f1ee4f1078afb47f1d012ad8a347e752817db))
- Move ActionSchedulerListener to ActionStateResult ([#1237](https://github.com/TraceMachina/nativelink/issues/1237)) - ([d57ee8d](https://github.com/TraceMachina/nativelink/commit/d57ee8d267e2a088f0f7f73c1108109b22ac1da0))
- modified the lre file path ([#1239](https://github.com/TraceMachina/nativelink/issues/1239)) - ([33f09cb](https://github.com/TraceMachina/nativelink/commit/33f09cbd1b2833956ffb268f786a7c035f375dae))
- Remove ClientOperationId and move all to OperationId ([#1214](https://github.com/TraceMachina/nativelink/issues/1214)) - ([81db90e](https://github.com/TraceMachina/nativelink/commit/81db90e17ddee6834e186f26c2395e6affda3799))
- Remove unnecessary sync trait bounds. ([#1227](https://github.com/TraceMachina/nativelink/issues/1227)) - ([e26e1b5](https://github.com/TraceMachina/nativelink/commit/e26e1b52274f0c4780dbd648c328dc57e30b75f2))
- Migrate from `redis-rs` to `fred.rs` ([#1188](https://github.com/TraceMachina/nativelink/issues/1188)) - ([44a4a91](https://github.com/TraceMachina/nativelink/commit/44a4a91e2e07dc21666c1c4afe96785dca3fac7a))
- Convert AwaitedAction to and from raw bytes ([#1206](https://github.com/TraceMachina/nativelink/issues/1206)) - ([f004351](https://github.com/TraceMachina/nativelink/commit/f004351d4235e1a37baae49260f2f1006472ac16))
- Make Cargo.toml feature pins compatible with project/main ([#1212](https://github.com/TraceMachina/nativelink/issues/1212)) - ([d8c407a](https://github.com/TraceMachina/nativelink/commit/d8c407a973a268e9a45078f2d5fe873f3e33b050))
- Remove unused features in dependencies ([#1211](https://github.com/TraceMachina/nativelink/issues/1211)) - ([a501971](https://github.com/TraceMachina/nativelink/commit/a501971f7da68c30768e7e36adbd1976ea43fbfc))
- ExistenceCacheStore now only evicts based on insert ([#1203](https://github.com/TraceMachina/nativelink/issues/1203)) - ([250037f](https://github.com/TraceMachina/nativelink/commit/250037f36212cc5c15c3ad2c928bc12fef20df2d))
- Remove unused dependencies ([#1207](https://github.com/TraceMachina/nativelink/issues/1207)) - ([df5f9e2](https://github.com/TraceMachina/nativelink/commit/df5f9e2422942a5d88e50acb3cf20e18b6c119c5))
- Migrate to hyper 1.x, axum 0.7.x, tonic 0.12.x ([#1155](https://github.com/TraceMachina/nativelink/issues/1155)) - ([532d1b1](https://github.com/TraceMachina/nativelink/commit/532d1b167da87f1cd0846506f396272c8c22aeff))
- S3 store can ignore `.has()` requests based on LastModified ([#1205](https://github.com/TraceMachina/nativelink/issues/1205)) - ([e874baa](https://github.com/TraceMachina/nativelink/commit/e874baad36c1d5e3c40edddbbc74022bf4250602))
- [Refactor] Complete metrics overhaul ([#1192](https://github.com/TraceMachina/nativelink/issues/1192)) - ([a6ff968](https://github.com/TraceMachina/nativelink/commit/a6ff968dc1963b89758df54f45c281e69c3a4e9d))
- Migrate to callPackage syntax ([#1193](https://github.com/TraceMachina/nativelink/issues/1193)) - ([534a102](https://github.com/TraceMachina/nativelink/commit/534a102021b643d0554395e7afbce63a0d3a0337))
- Implement Serialize/Deserialize for ActionStage ([#1186](https://github.com/TraceMachina/nativelink/issues/1186)) - ([3574149](https://github.com/TraceMachina/nativelink/commit/357414918c4addeecd71e1c316484cadd899fd31))
- update store_trait.rs ([#1184](https://github.com/TraceMachina/nativelink/issues/1184)) - ([97f64b2](https://github.com/TraceMachina/nativelink/commit/97f64b24a15462d5b4b2d8b7efffa089ef93e143))
- Double protect output stream of verify store ([#1180](https://github.com/TraceMachina/nativelink/issues/1180)) - ([e6542e6](https://github.com/TraceMachina/nativelink/commit/e6542e67cc68d1f2873858cccc51b5642b1b5f27))
- Make TaskExecutor a wrapper around TokioExecutor ([#1159](https://github.com/TraceMachina/nativelink/issues/1159)) - ([b7ef3b6](https://github.com/TraceMachina/nativelink/commit/b7ef3b6c7af2451fafc8690158d49769b3d31dc8))
- Increase chromium deployment example jobs size ([#1146](https://github.com/TraceMachina/nativelink/issues/1146)) - ([0e265dc](https://github.com/TraceMachina/nativelink/commit/0e265dcde4471e46782ae57764b60dc68c4d8c57))
- Refresh readme ([#1078](https://github.com/TraceMachina/nativelink/issues/1078)) - ([414289a](https://github.com/TraceMachina/nativelink/commit/414289a3eedfaf32e82658e16f4ab238d680fb8b))
- Change remote cache URLs from secrets to vars ([#1143](https://github.com/TraceMachina/nativelink/issues/1143)) - ([6e37f47](https://github.com/TraceMachina/nativelink/commit/6e37f4780152d9d5db06775409298a781b3e3d2a))
- converted single defaults from plural ([#1099](https://github.com/TraceMachina/nativelink/issues/1099)) - ([0a05082](https://github.com/TraceMachina/nativelink/commit/0a05082342f69a6f64a5d49f24152cbd8fac0821))
- Write Tekton image tag outputs to a ConfigMap ([#1100](https://github.com/TraceMachina/nativelink/issues/1100)) - ([1b8e23b](https://github.com/TraceMachina/nativelink/commit/1b8e23b6342ea73b1b49059addf5f6a290517989))
- Temporarily disable rustdoc autogen ([#1101](https://github.com/TraceMachina/nativelink/issues/1101)) - ([3aa4f94](https://github.com/TraceMachina/nativelink/commit/3aa4f94af2b34ef9e9d331429438b778789433b6))
- Cancel running GHA workflows on pushes to the same branch ([#1090](https://github.com/TraceMachina/nativelink/issues/1090)) - ([545f752](https://github.com/TraceMachina/nativelink/commit/545f752d10f86c493efce3a04e073c739e604479))
- Make bystream limits configurable ([#1076](https://github.com/TraceMachina/nativelink/issues/1076)) - ([54a9345](https://github.com/TraceMachina/nativelink/commit/54a93453deb21df2d4c7489b43596e6539814554))
- [Refactor] Workers::find_worker_for_action should take PlatformProperties ([#1068](https://github.com/TraceMachina/nativelink/issues/1068)) - ([f5e7276](https://github.com/TraceMachina/nativelink/commit/f5e72760e722a34023e9196073d23fc38443e5ef))
- Include ActionState to MatchingEngineActionStateResult ([#1064](https://github.com/TraceMachina/nativelink/issues/1064)) - ([35e9cd7](https://github.com/TraceMachina/nativelink/commit/35e9cd71851ba15c09e9a1d71907feb51337419b))
- revert bazel version bump. ([#1061](https://github.com/TraceMachina/nativelink/issues/1061)) - ([194ab78](https://github.com/TraceMachina/nativelink/commit/194ab78827a6f64d361037f9cc2c069363cf1638))
- Remove `#[async_trait]` where possible ([#620](https://github.com/TraceMachina/nativelink/issues/620)) ([#1055](https://github.com/TraceMachina/nativelink/issues/1055)) - ([ba168a3](https://github.com/TraceMachina/nativelink/commit/ba168a3bafdbe123691667aad58bc1af3ee875e1))
- Rename cas CompressionAlgorithm to HttpCompressionAlgorithm ([#1052](https://github.com/TraceMachina/nativelink/issues/1052)) - ([9ba4323](https://github.com/TraceMachina/nativelink/commit/9ba43236cf61737cd9561a1657ee50686b459966))
- Implement MatchingEngineStateManager ([#1041](https://github.com/TraceMachina/nativelink/issues/1041)) - ([684dbc1](https://github.com/TraceMachina/nativelink/commit/684dbc1c6bf8d1c77b97dc3fc945daf9c5a5d3d6))
- Move `update_action_with_internal_error` into `StateManager` ([#1053](https://github.com/TraceMachina/nativelink/issues/1053)) - ([0f33a8a](https://github.com/TraceMachina/nativelink/commit/0f33a8aebf4509fef2f1172ad6626ce267482d6b))
- Implement WorkerStateManager for simple scheduler ([#993](https://github.com/TraceMachina/nativelink/issues/993)) - ([1359513](https://github.com/TraceMachina/nativelink/commit/1359513f5fc8f51856e8bcdbd55c9eb5c06131e1))
- Remove execution permissions from non-executable files ([#1048](https://github.com/TraceMachina/nativelink/issues/1048)) - ([fbc39f5](https://github.com/TraceMachina/nativelink/commit/fbc39f58d1fa240731fa5d08aafcc1ede54fe885))
- Sync serde version in Cargo.toml to lockfile ([#966](https://github.com/TraceMachina/nativelink/issues/966)) - ([59df55d](https://github.com/TraceMachina/nativelink/commit/59df55d0e52cbf8a7f9bc4b12e2f5f3a480ea17f))
- Support cluster mode when using Redis as a store ([#998](https://github.com/TraceMachina/nativelink/issues/998)) - ([c85b6df](https://github.com/TraceMachina/nativelink/commit/c85b6df457395d7fa8aeb121ad1b7ea69b3f65ae))
- Implement `ClientStateManager` for `SimpleScheduler` ([#985](https://github.com/TraceMachina/nativelink/issues/985)) - ([49efde2](https://github.com/TraceMachina/nativelink/commit/49efde28cc0828b771472cfc6f2f2cbfd2acc2cc))
- Reduce native-cli executable size ([#1010](https://github.com/TraceMachina/nativelink/issues/1010)) - ([d1a8d9d](https://github.com/TraceMachina/nativelink/commit/d1a8d9d8a580c9298018918c9bf3aa887da33f8b))
- Sync Cargo MSRV to Bazel ([#1011](https://github.com/TraceMachina/nativelink/issues/1011)) - ([c0b284d](https://github.com/TraceMachina/nativelink/commit/c0b284d5a2183eea6f4d3c3c699ad633e97fc75d))
- [Refactor] Stores now return Arc for construction ([#989](https://github.com/TraceMachina/nativelink/issues/989)) - ([5bdc9eb](https://github.com/TraceMachina/nativelink/commit/5bdc9ebfb558631f93763fceb5cfd88be359a25a))
- Enable the dotcom workflow on main ([#1008](https://github.com/TraceMachina/nativelink/issues/1008)) - ([28314e4](https://github.com/TraceMachina/nativelink/commit/28314e4c7a5072b219f60bd455453273a67f26e1))
- EvictingMap now supports B-tree lookups ([#996](https://github.com/TraceMachina/nativelink/issues/996)) - ([fd4c89c](https://github.com/TraceMachina/nativelink/commit/fd4c89cf6ac772dfbab4965135c84d6ff29671ad))
- [refactor] Migrate `worker::WorkerId` for `action_messages::WorkerId` ([#992](https://github.com/TraceMachina/nativelink/issues/992)) - ([50401c3](https://github.com/TraceMachina/nativelink/commit/50401c3a9b9b88bbe3ca7ce9debb9c2afcc70b2c))
- [Refactor] Simple scheduler method signatures to async ([#971](https://github.com/TraceMachina/nativelink/issues/971)) - ([3c50dd5](https://github.com/TraceMachina/nativelink/commit/3c50dd5c42c925902931ae3da65179f2e465c838))
- Refactor Store API to use StoreKey ([#964](https://github.com/TraceMachina/nativelink/issues/964)) - ([e524bbc](https://github.com/TraceMachina/nativelink/commit/e524bbc7291612c4d2355f0742c713cbbbf20122))
- Refactor Store Api into client side and driver side ([#935](https://github.com/TraceMachina/nativelink/issues/935)) - ([04beafd](https://github.com/TraceMachina/nativelink/commit/04beafd49a4bc4520527f025750d209c64d61dfa))
- Create New Glossary ([#957](https://github.com/TraceMachina/nativelink/issues/957)) - ([77b2c33](https://github.com/TraceMachina/nativelink/commit/77b2c333cd0ed70814cc94f53427090ab5ff7ada))
- Use single quotes for char ([#955](https://github.com/TraceMachina/nativelink/issues/955)) - ([e90c4bc](https://github.com/TraceMachina/nativelink/commit/e90c4bc6811ecd2ee3b4e0a48f0df76faf53035a))
- Include UUID in ActionState ([#927](https://github.com/TraceMachina/nativelink/issues/927)) - ([b07ca1d](https://github.com/TraceMachina/nativelink/commit/b07ca1d3514f2ea10fd62cd3688a14789318e03e))
- Refactor EvictingMap so it does not use DigestInfo ([#932](https://github.com/TraceMachina/nativelink/issues/932)) - ([9c45e86](https://github.com/TraceMachina/nativelink/commit/9c45e864be52718946c180627807009089036141))

### ⬆️ Bumps & Version Updates

- Bump Go deps ([#1219](https://github.com/TraceMachina/nativelink/issues/1219)) - ([a953f19](https://github.com/TraceMachina/nativelink/commit/a953f19946849a8272f4437c5f767f13e4a7b468))
- Upgrade toolchains ([#1191](https://github.com/TraceMachina/nativelink/issues/1191)) - ([97135e9](https://github.com/TraceMachina/nativelink/commit/97135e9ed8510c347868ae3e81bd52973cc0a987))
- Bump some Bazel deps ([#1176](https://github.com/TraceMachina/nativelink/issues/1176)) - ([f9ef39c](https://github.com/TraceMachina/nativelink/commit/f9ef39c09d7f5f54072e45d43e79b3ac86399009))
- Update copyright headers ([#1172](https://github.com/TraceMachina/nativelink/issues/1172)) - ([02465d3](https://github.com/TraceMachina/nativelink/commit/02465d3a185d9b1e651bdf9e27aabfb54981835c))
- Update Go dependencies ([#1095](https://github.com/TraceMachina/nativelink/issues/1095)) - ([98d645f](https://github.com/TraceMachina/nativelink/commit/98d645fc15fdae6cb5d3e25c6383280acbe04e5e))
- Update Rust crate uuid to v1.9.0 ([#1050](https://github.com/TraceMachina/nativelink/issues/1050)) - ([62f5a90](https://github.com/TraceMachina/nativelink/commit/62f5a901f771143c2c306a34e224ca84cd794b58))
- Update Rust crate mimalloc to v0.1.43 ([#1047](https://github.com/TraceMachina/nativelink/issues/1047)) - ([b6d2035](https://github.com/TraceMachina/nativelink/commit/b6d20352dcaab0e65b3d01bb2f96b1216d7c4d2e))
- Update Rust crate syn to v2.0.68 ([#1046](https://github.com/TraceMachina/nativelink/issues/1046)) - ([97abbcd](https://github.com/TraceMachina/nativelink/commit/97abbcd24b4f87f500f6ab2d9898b4a8401d9f3b))
- Update Rust crate proc-macro2 to v1.0.86 ([#1045](https://github.com/TraceMachina/nativelink/issues/1045)) - ([f830294](https://github.com/TraceMachina/nativelink/commit/f8302942b4f8ed94210913f0e82dac59fe89d1f9))
- Update aws-sdk-rust monorepo ([#1042](https://github.com/TraceMachina/nativelink/issues/1042)) - ([5f8a4f2](https://github.com/TraceMachina/nativelink/commit/5f8a4f2e8087210cdbb02f1cbe591436449e051f))
- Update dependency rules_java to v7.6.5 ([#1040](https://github.com/TraceMachina/nativelink/issues/1040)) - ([cc53957](https://github.com/TraceMachina/nativelink/commit/cc53957b16da67482a44fcec472b53e4cfe7bd54))
- Update dependency rules_rust to v0.46.0 ([#1037](https://github.com/TraceMachina/nativelink/issues/1037)) - ([47a25b8](https://github.com/TraceMachina/nativelink/commit/47a25b87e2c9159fcf9d93fd28e62e59e5684f65))
- Update dependency rules_python to v0.33.2 ([#1036](https://github.com/TraceMachina/nativelink/issues/1036)) - ([6049d35](https://github.com/TraceMachina/nativelink/commit/6049d355df085b8c6c32045a82879ca8e96abd6d))
- Update dependency rules_java to v7.6.4 ([#1035](https://github.com/TraceMachina/nativelink/issues/1035)) - ([7c52e89](https://github.com/TraceMachina/nativelink/commit/7c52e89adb9c5bd180b0fc6f2e1802afef9634ec))
- Update dependency bazel to v7.2.0 ([#1033](https://github.com/TraceMachina/nativelink/issues/1033)) - ([a675de6](https://github.com/TraceMachina/nativelink/commit/a675de61c360b4d8af6c8c965dfb30602d1b2a04))
- Update dependency protobuf to v27.1.bcr.1 ([#1034](https://github.com/TraceMachina/nativelink/issues/1034)) - ([1bc0f1a](https://github.com/TraceMachina/nativelink/commit/1bc0f1ae485dad24f4483d289f4d776c4f8f582b))
- Update Rust crate console-subscriber to 0.3.0 ([#1032](https://github.com/TraceMachina/nativelink/issues/1032)) - ([b49bc26](https://github.com/TraceMachina/nativelink/commit/b49bc26a4fff2a68a8832766ced7486cf6fca9bb))
- Update Rust crate async-lock to v3.4.0 ([#1031](https://github.com/TraceMachina/nativelink/issues/1031)) - ([c247057](https://github.com/TraceMachina/nativelink/commit/c247057a8ad62277ff0c9fbe4ba533d1319c07c8))
- Update Rust crate proc-macro2 to v1.0.85 ([#1029](https://github.com/TraceMachina/nativelink/issues/1029)) - ([90da4c9](https://github.com/TraceMachina/nativelink/commit/90da4c92f62270d31a1525beaff96a3832a71eae))
- Update Rust crate hyper to v0.14.29 ([#1028](https://github.com/TraceMachina/nativelink/issues/1028)) - ([0a64bb1](https://github.com/TraceMachina/nativelink/commit/0a64bb1c5a44ef280b3ead76ad93c29f1f7d86a8))
- Update aws-sdk-rust monorepo ([#1030](https://github.com/TraceMachina/nativelink/issues/1030)) - ([fc656de](https://github.com/TraceMachina/nativelink/commit/fc656deeb2b8b8cf62a3219d25e1812abbcb3f56))
- Update Rust crate clap to v4.5.7 ([#1026](https://github.com/TraceMachina/nativelink/issues/1026)) - ([9c0c68a](https://github.com/TraceMachina/nativelink/commit/9c0c68aeb7a8b94229512d121e70a845da04a7c2))
- Update git & remove unused deps in ubuntu runners ([#1024](https://github.com/TraceMachina/nativelink/issues/1024)) - ([b71952b](https://github.com/TraceMachina/nativelink/commit/b71952b0650aa9537759dc8d3bdc37bf3d430769))
- Bump yarn deps ([#1015](https://github.com/TraceMachina/nativelink/issues/1015)) - ([b2678ff](https://github.com/TraceMachina/nativelink/commit/b2678ff961ab653ef31ced06d7036934ff478f61))
- Update `Vale` CI action to handle large diffs ([#978](https://github.com/TraceMachina/nativelink/issues/978)) - ([f4ce898](https://github.com/TraceMachina/nativelink/commit/f4ce898266173a294275b8fdabf7e2d8e18f0c1c))
- Increase pre-commit timeout in CI ([#956](https://github.com/TraceMachina/nativelink/issues/956)) - ([9bebba8](https://github.com/TraceMachina/nativelink/commit/9bebba812e7c05ba6476da86095ae151d5be42f9))
- Bump trivially bumpable deps ([#950](https://github.com/TraceMachina/nativelink/issues/950)) - ([5ecc739](https://github.com/TraceMachina/nativelink/commit/5ecc739785b07370181ad0ab408aac50957e3b20))
- Bump flake and Bazel modules ([#947](https://github.com/TraceMachina/nativelink/issues/947)) - ([0eed759](https://github.com/TraceMachina/nativelink/commit/0eed7593b1a55ed9998569764080ea2c1b3406a4))
- Update Rust crate syn to v2.0.66 ([#946](https://github.com/TraceMachina/nativelink/issues/946)) - ([80af57f](https://github.com/TraceMachina/nativelink/commit/80af57f409f4d3cf67ecd616f197190fd78bf52b))
- Update Rust crate redis to v0.25.4 ([#944](https://github.com/TraceMachina/nativelink/issues/944)) - ([5fbd751](https://github.com/TraceMachina/nativelink/commit/5fbd751d2ec7e9866a84ee8ce65701bd507555c1))
- Update Rust crate quote to v1.0.36 ([#938](https://github.com/TraceMachina/nativelink/issues/938)) - ([0300a12](https://github.com/TraceMachina/nativelink/commit/0300a128a2facaad80c4c24db0dbc1b47ccca5b1))
- Update dependency protobuf to v26.0.bcr.1 ([#887](https://github.com/TraceMachina/nativelink/issues/887)) - ([724693f](https://github.com/TraceMachina/nativelink/commit/724693f0d386e24e87e4b87158925c0281edea53))
- Update Rust crate parking_lot to v0.12.3 ([#936](https://github.com/TraceMachina/nativelink/issues/936)) - ([fd643e6](https://github.com/TraceMachina/nativelink/commit/fd643e6826a83f31e48e0de4add2ee1b7a9d5caf))
- Update Rust crate mimalloc to v0.1.42 ([#933](https://github.com/TraceMachina/nativelink/issues/933)) - ([08e2f2e](https://github.com/TraceMachina/nativelink/commit/08e2f2ec2ed9dc9b840bb2d23ab640291eaaf8a6))
- Update Rust crate proc-macro2 to v1.0.84 ([#916](https://github.com/TraceMachina/nativelink/issues/916)) - ([409af67](https://github.com/TraceMachina/nativelink/commit/409af67fc6093f87a4240abc83768946872d528d))

## [0.4.0](https://github.com/TraceMachina/nativelink/compare/v0.3.2..v0.4.0) - 2024-05-16



### ❌️  Breaking Changes

- [Breaking] Factor out health status checks to its own service ([#823](https://github.com/TraceMachina/nativelink/issues/823)) - ([ea50856](https://github.com/TraceMachina/nativelink/commit/ea508561d8faf1de3a7188867c70b7ef36069572))

### ⛰️  Features

- Implement get_tree() feature ([#905](https://github.com/TraceMachina/nativelink/issues/905)) - ([ae44878](https://github.com/TraceMachina/nativelink/commit/ae448781e8ab3f0fa4d0e60d0ddd446d5ba51107))
- Introduce the LRE flake module ([#909](https://github.com/TraceMachina/nativelink/issues/909)) - ([60f712b](https://github.com/TraceMachina/nativelink/commit/60f712bcddd5c2cd3d3bdd537c4cc136fe6497c7))
- Add OriginContext to track data across modules ([#875](https://github.com/TraceMachina/nativelink/issues/875)) - ([829904e](https://github.com/TraceMachina/nativelink/commit/829904eed7a42f72d7b1a951effde436b68f2b4c))
- Add backend store metrics to VerifyStore ([#897](https://github.com/TraceMachina/nativelink/issues/897)) - ([7effcc4](https://github.com/TraceMachina/nativelink/commit/7effcc41f9977a370658c0b43e547551cf873b47))
- Add metrics to CompletenessCheckingStore ([#882](https://github.com/TraceMachina/nativelink/issues/882)) - ([520b762](https://github.com/TraceMachina/nativelink/commit/520b762e513dbac0d1a58c4172b31bd10cdfdaed))
- Add hit metrics to FastSlowStore ([#884](https://github.com/TraceMachina/nativelink/issues/884)) - ([6c9071f](https://github.com/TraceMachina/nativelink/commit/6c9071f52d55343ca811aa8941ab8379ba6c930d))
- Add metrics output to SizePartitioningStore ([#880](https://github.com/TraceMachina/nativelink/issues/880)) - ([17ecf8a](https://github.com/TraceMachina/nativelink/commit/17ecf8afe6da1f6e23f8e2a199cfc5bd663bd8d0))
- Allow K8s demos to use prebuilt images ([#872](https://github.com/TraceMachina/nativelink/issues/872)) - ([24e30fa](https://github.com/TraceMachina/nativelink/commit/24e30fa85e86e9e31d2f724438948e244c307290))
- Add Redis Store ([#393](https://github.com/TraceMachina/nativelink/issues/393)) - ([f79b59b](https://github.com/TraceMachina/nativelink/commit/f79b59beee449762742482890cb76eef172c9d8a))
- Introduce the `native` CLI ([#851](https://github.com/TraceMachina/nativelink/issues/851)) - ([fbe0583](https://github.com/TraceMachina/nativelink/commit/fbe0583324fd7952a96e9df1f8bf622a70272525))
- Refactor buf_channel ([#849](https://github.com/TraceMachina/nativelink/issues/849)) - ([f5e0035](https://github.com/TraceMachina/nativelink/commit/f5e0035c7fa07e25b724c98a9295c9593645369b))

### 🐛 Bug Fixes

- Fix possible deadlock if max_open_files set too low ([#908](https://github.com/TraceMachina/nativelink/issues/908)) - ([e0a7bb9](https://github.com/TraceMachina/nativelink/commit/e0a7bb991ff3947fe7294d5e14940433375f9a0c))
- Fix LLVM 18 toolchains after fb0edae ([#883](https://github.com/TraceMachina/nativelink/issues/883)) - ([8ee7ab3](https://github.com/TraceMachina/nativelink/commit/8ee7ab346f47800ab4cc6ebf3098236840c4ecd8))
- Migrate K8s HTTPRoutes to GRPCRoutes ([#868](https://github.com/TraceMachina/nativelink/issues/868)) - ([7e379ff](https://github.com/TraceMachina/nativelink/commit/7e379fff80dcd2653b5cb21c1ae1bd4a488a86c9))
- Fix bug in buf_channel::consume() where exact size doesn't receive eof ([#858](https://github.com/TraceMachina/nativelink/issues/858)) - ([5583a5d](https://github.com/TraceMachina/nativelink/commit/5583a5d5cd825fe7070fd84311331fa10bc47318))
- Fix semver image workflow after 646253d ([#844](https://github.com/TraceMachina/nativelink/issues/844)) - ([e890c01](https://github.com/TraceMachina/nativelink/commit/e890c01c1e4654b9b2aae026614f005be06de117))

### 📚 Documentation

- Update README.md (small edits) ([#903](https://github.com/TraceMachina/nativelink/issues/903)) - ([727fd19](https://github.com/TraceMachina/nativelink/commit/727fd199dfce54c7931febc25237556a5c2016b7))
- Update Chromium Readme ([#896](https://github.com/TraceMachina/nativelink/issues/896)) - ([185eab3](https://github.com/TraceMachina/nativelink/commit/185eab3e25c07ba253785a72520c122069e6e9f0))
- Update README.md to pin version ([#873](https://github.com/TraceMachina/nativelink/issues/873)) - ([73c9929](https://github.com/TraceMachina/nativelink/commit/73c9929a17839be605af988380fb453646cd1c1a))
- Rewrite contribution documentation ([#827](https://github.com/TraceMachina/nativelink/issues/827)) - ([5e4c32c](https://github.com/TraceMachina/nativelink/commit/5e4c32cce05d592ab3bcdfd75cbfb14b29551045))
- Warn people about Nix in Chrome README.md ([#865](https://github.com/TraceMachina/nativelink/issues/865)) - ([d381162](https://github.com/TraceMachina/nativelink/commit/d381162dc8f628171f3c7ea4fc6707ac303d036d))
- Update Kubernetes Readme ([#846](https://github.com/TraceMachina/nativelink/issues/846)) - ([4082759](https://github.com/TraceMachina/nativelink/commit/4082759e86d28c8edef95108a210c3b0aa362508))
- Document release process ([#847](https://github.com/TraceMachina/nativelink/issues/847)) - ([d854874](https://github.com/TraceMachina/nativelink/commit/d854874efdf3044894270e8c69bda26f8b885270))

### 🧪 Testing & CI

- Test building with Nix ([#920](https://github.com/TraceMachina/nativelink/issues/920)) - ([3391fdf](https://github.com/TraceMachina/nativelink/commit/3391fdf7074e790fbac72774947b333797385fa3))
- Harden CI against too long running jobs ([#917](https://github.com/TraceMachina/nativelink/issues/917)) - ([ba7ed50](https://github.com/TraceMachina/nativelink/commit/ba7ed50e5d297500ddd8bb4a7f5d975c32a17c2e))
- Fix operations scripts evaluating to quickly ([#906](https://github.com/TraceMachina/nativelink/issues/906)) - ([66a72ab](https://github.com/TraceMachina/nativelink/commit/66a72ab4cc21bccdc2997cd0b2600ba503c0a424))
- Add nativelink_test macro for tests ([#888](https://github.com/TraceMachina/nativelink/issues/888)) - ([c0d7eaa](https://github.com/TraceMachina/nativelink/commit/c0d7eaa4f898bb13c90c2ed05b1ed6ae366e0797))

### ⚙️ Miscellaneous

- Reduce keep alive log message level ([#894](https://github.com/TraceMachina/nativelink/issues/894)) - ([f9e67aa](https://github.com/TraceMachina/nativelink/commit/f9e67aa1ba77f2a077153561afd1624bbfc502d8))
- Migrate to Bazelisk ([#912](https://github.com/TraceMachina/nativelink/issues/912)) - ([ab46197](https://github.com/TraceMachina/nativelink/commit/ab46197a0a88ade04db8e142296ea99f0fdb29b3))
- Enable hermetic Bazel sandboxing ([#902](https://github.com/TraceMachina/nativelink/issues/902)) - ([acec6d3](https://github.com/TraceMachina/nativelink/commit/acec6d3792f27f031c765aa0f38fee920dff2b06))
- All tokio::spawn and related functions must use nativelink's version ([#890](https://github.com/TraceMachina/nativelink/issues/890)) - ([c1d0402](https://github.com/TraceMachina/nativelink/commit/c1d040277cfb7cbb252d57c07a427574ed314e92))
- Remove zig-cc ([#876](https://github.com/TraceMachina/nativelink/issues/876)) - ([402f335](https://github.com/TraceMachina/nativelink/commit/402f335d8a9a12e09691282903fc8631896203dd))
- Migrate all logging to the tracing library ([#871](https://github.com/TraceMachina/nativelink/issues/871)) - ([523ee33](https://github.com/TraceMachina/nativelink/commit/523ee33784c2dfdd5a988cdf3cb4843a66d92244))
- Refactor S3 store & support upload retry ([#854](https://github.com/TraceMachina/nativelink/issues/854)) - ([9db29ef](https://github.com/TraceMachina/nativelink/commit/9db29ef3e5c9875d52519ae18198739e6baa6aa4))
- fix a typo in the script comments. ([#856](https://github.com/TraceMachina/nativelink/issues/856)) - ([6d45a00](https://github.com/TraceMachina/nativelink/commit/6d45a0057781af0083d3f6a0c19065d10c762993))
- Rename buf_channel::take() to buf_channel::consume() ([#848](https://github.com/TraceMachina/nativelink/issues/848)) - ([aadb2b9](https://github.com/TraceMachina/nativelink/commit/aadb2b9d89bd42eba7791b5d31c5cdeb75e90087))
- Connection Manager Rewrite ([#806](https://github.com/TraceMachina/nativelink/issues/806)) - ([a842f3a](https://github.com/TraceMachina/nativelink/commit/a842f3a8bbbfe6145c1935b39264be85272bbe6a))

### ⬆️ Bumps & Version Updates

- Bump trivially bumpable deps ([#914](https://github.com/TraceMachina/nativelink/issues/914)) - ([0ff1f45](https://github.com/TraceMachina/nativelink/commit/0ff1f45640b646102f43acaf7d911db0b0d5cc06))
- Update all development dependencies ([#910](https://github.com/TraceMachina/nativelink/issues/910)) - ([8a63295](https://github.com/TraceMachina/nativelink/commit/8a632953b86395088e4ab8c1e160a650739549b7))
- Bump cilium in devcluster to 1.16.0-pre.2 ([#904](https://github.com/TraceMachina/nativelink/issues/904)) - ([64ed20a](https://github.com/TraceMachina/nativelink/commit/64ed20a40964b8c606c7d65f76af840bcfc837fd))
- Update dependency platforms to v0.0.10 ([#886](https://github.com/TraceMachina/nativelink/issues/886)) - ([7f799d7](https://github.com/TraceMachina/nativelink/commit/7f799d72cb5f18b48a861304fa86846ea357331a))
- Update Nix installers in CI ([#879](https://github.com/TraceMachina/nativelink/issues/879)) - ([5a549ba](https://github.com/TraceMachina/nativelink/commit/5a549bacbf23d1df07811cc71f3beb8dc0e30859))
- Update Rust crate parking_lot to 0.12.2 ([#885](https://github.com/TraceMachina/nativelink/issues/885)) - ([f6e02a6](https://github.com/TraceMachina/nativelink/commit/f6e02a6ee0a33bbec6fb1581f664f293f67efd27))
- Update dependency clsx to v2.1.1 ([#878](https://github.com/TraceMachina/nativelink/issues/878)) - ([7227649](https://github.com/TraceMachina/nativelink/commit/7227649dd31cabcb999e9632a1563211b46206d5))
- Bump trivially bumpable deps ([#877](https://github.com/TraceMachina/nativelink/issues/877)) - ([fb0edae](https://github.com/TraceMachina/nativelink/commit/fb0edae71180d435d0c3de46a245953c71702222))
- Update Rust version to 1.77.2 ([#857](https://github.com/TraceMachina/nativelink/issues/857)) - ([b2b83df](https://github.com/TraceMachina/nativelink/commit/b2b83df0775e1d02c6a9725263c9b4edda99da6a))
- Update Rust crate rustls-pemfile to 2.1.2 ([#852](https://github.com/TraceMachina/nativelink/issues/852)) - ([44bc15f](https://github.com/TraceMachina/nativelink/commit/44bc15f54647903b698ff96816e30776936ca03a))
- Update Rust crate async-trait to 0.1.80 ([#850](https://github.com/TraceMachina/nativelink/issues/850)) - ([8df4345](https://github.com/TraceMachina/nativelink/commit/8df4345a4b5a72a30e8c1d64d4b762b8ea3bf80c))

## [0.3.2](https://github.com/TraceMachina/nativelink/compare/v0.2.0..v0.3.2) - 2024-04-09



### ❌️  Breaking Changes

- [Breaking] Remove completeness checking logic in CacheLookupScheduler - ([692e4de](https://github.com/TraceMachina/nativelink/commit/692e4de6c44ce070b448235428736d9d73eea997))
- [Breaking] Generalize LRE to arbitrary toolchains ([#728](https://github.com/TraceMachina/nativelink/issues/728)) - ([1a43ef9](https://github.com/TraceMachina/nativelink/commit/1a43ef91c8587b5c4708643f1593968286586f01))
- [Breaking] Change in behavior of /status by introduction of component based health ([#636](https://github.com/TraceMachina/nativelink/issues/636)) - ([48cadc7](https://github.com/TraceMachina/nativelink/commit/48cadc74c886b0d102a016656e6d8cda3adea0c2))
- [BREAKING] Add concurrency limit to GRPC ([#627](https://github.com/TraceMachina/nativelink/issues/627)) - ([b47f39b](https://github.com/TraceMachina/nativelink/commit/b47f39ba9951fe8de554fe2725fc16136cfe8699))
- [Breaking] Deny unknown fields durning configuration serialization ([#603](https://github.com/TraceMachina/nativelink/issues/603)) - ([95afd36](https://github.com/TraceMachina/nativelink/commit/95afd3627b9a4782705a3ef8097c151a6aea130c))

### ⛰️  Features

- Add safe request timeout for running actions manager ([#743](https://github.com/TraceMachina/nativelink/issues/743)) - ([33db963](https://github.com/TraceMachina/nativelink/commit/33db963faaaf5826c5da08e7bf96c9fab71d1fe8))
- Implement worker api for killing running actions ([#840](https://github.com/TraceMachina/nativelink/issues/840)) - ([abf12e8](https://github.com/TraceMachina/nativelink/commit/abf12e8ee238d9f9d279bd601d23625fd5c72a67))
- Create directory for action ([#752](https://github.com/TraceMachina/nativelink/issues/752)) - ([414fff3](https://github.com/TraceMachina/nativelink/commit/414fff35ef82259a434dbdb14c13036a0d22c9c4))
- Add nativelink-debug target ([#811](https://github.com/TraceMachina/nativelink/issues/811)) - ([c60fb55](https://github.com/TraceMachina/nativelink/commit/c60fb556eba65e492c8c2ebad038d6f2940d9239))
- Allow variables in platform property values ([#809](https://github.com/TraceMachina/nativelink/issues/809)) - ([09fc7f8](https://github.com/TraceMachina/nativelink/commit/09fc7f8561568e0e7a1500b069d64e6499421a66))
- Use mimalloc as global memory allocator ([#749](https://github.com/TraceMachina/nativelink/issues/749)) - ([6c647d6](https://github.com/TraceMachina/nativelink/commit/6c647d68e2bdc349fad0a67de6b05a1a91aeb031))
- Optimize file uploads when source is file ([#723](https://github.com/TraceMachina/nativelink/issues/723)) - ([7c9a070](https://github.com/TraceMachina/nativelink/commit/7c9a07085298d1546b4459d6a22ec87bf8189395))
- Add API so stores can get Arc<Store> or &Store ([#679](https://github.com/TraceMachina/nativelink/issues/679)) - ([5df8a78](https://github.com/TraceMachina/nativelink/commit/5df8a780fc099e9b594f7dfd92f0ed59ffadd95c))
- Add check for slow store to be noop and conditionally replace with fast ([#670](https://github.com/TraceMachina/nativelink/issues/670)) - ([e402a10](https://github.com/TraceMachina/nativelink/commit/e402a10d113fada3f73918090b9c58521b225011))
- Max concurrent GrpcStore streams ([#656](https://github.com/TraceMachina/nativelink/issues/656)) - ([7548d4b](https://github.com/TraceMachina/nativelink/commit/7548d4b58e967e665df029d1df7b79f81f9d15e2))
- Add metrics to compression and existence cache store ([#651](https://github.com/TraceMachina/nativelink/issues/651)) - ([722c80b](https://github.com/TraceMachina/nativelink/commit/722c80bc50149210f064fadb52f1ad04bf9197db))
- Retry GrpcStore get_part_ref ([#646](https://github.com/TraceMachina/nativelink/issues/646)) - ([d46180c](https://github.com/TraceMachina/nativelink/commit/d46180c5f4ed548346c227a0e52ecc60994baf34))
- Allow ByteStream write restart ([#635](https://github.com/TraceMachina/nativelink/issues/635)) - ([3fabbaa](https://github.com/TraceMachina/nativelink/commit/3fabbaaeb1c029ce98d979acb58b5ec94af5c3a4))
- Add warning for TLS ([#609](https://github.com/TraceMachina/nativelink/issues/609)) - ([63e2ad6](https://github.com/TraceMachina/nativelink/commit/63e2ad6ce33dad11d6c88de5f6eea6cbd491b18f))
- Add support for mTLS ([#470](https://github.com/TraceMachina/nativelink/issues/470)) - ([6a379b3](https://github.com/TraceMachina/nativelink/commit/6a379b314ef3f4428f116f82d7af55e1e31ca7ac))
- Add S3 http2 toggle flag ([#604](https://github.com/TraceMachina/nativelink/issues/604)) - ([8c433cd](https://github.com/TraceMachina/nativelink/commit/8c433cdd443a2a4d420874171066b3f7d67a1790))
- Add blake3 support for verify store ([#575](https://github.com/TraceMachina/nativelink/issues/575)) - ([3acefc7](https://github.com/TraceMachina/nativelink/commit/3acefc73d87b4091fc399dfed4951dd8046626a3))
- Build nativelink with musl ([#583](https://github.com/TraceMachina/nativelink/issues/583)) - ([ee4846c](https://github.com/TraceMachina/nativelink/commit/ee4846c238780ce66a52fb7bce08bb7ee4d3e5bc))
- Shard store weight scale distribution ([#574](https://github.com/TraceMachina/nativelink/issues/574)) - ([928f12f](https://github.com/TraceMachina/nativelink/commit/928f12f81c5a5fefcb48385f6ba68e7a444cdca6))
- Add console subscriber ([#545](https://github.com/TraceMachina/nativelink/issues/545)) - ([bb30474](https://github.com/TraceMachina/nativelink/commit/bb3047493bccc795db9b64edd911ce85358d6d57))

### 🐛 Bug Fixes

- Resolve upload deadlock ([#816](https://github.com/TraceMachina/nativelink/issues/816)) - ([b61142d](https://github.com/TraceMachina/nativelink/commit/b61142dd9c9dc3e85d9adc8a23668f9ad234c128))
- Fix nightly clippy warnings ([#817](https://github.com/TraceMachina/nativelink/issues/817)) - ([6d87cca](https://github.com/TraceMachina/nativelink/commit/6d87cca55ef739c2253860885e53529e2084c498))
- Fix `.gitignore` after 1a43ef9 ([#797](https://github.com/TraceMachina/nativelink/issues/797)) - ([53e5a99](https://github.com/TraceMachina/nativelink/commit/53e5a99bd96491c75fce050fd290812cf47d7219))
- Fix image publishing workflow after 1a43ef9 ([#777](https://github.com/TraceMachina/nativelink/issues/777)) - ([54b21b8](https://github.com/TraceMachina/nativelink/commit/54b21b8512e7cf920c4c2d3e21110e7266fc7f27))
- Completeness checking store should not check if directory digests exist ([#748](https://github.com/TraceMachina/nativelink/issues/748)) - ([e979e31](https://github.com/TraceMachina/nativelink/commit/e979e31cce278989f9673e9b0fdb057b08d1af20))
- Check owner and group executable bits ([#727](https://github.com/TraceMachina/nativelink/issues/727)) - ([cea2336](https://github.com/TraceMachina/nativelink/commit/cea2336c20145d36202413ec55cbe95b71bbce36))
- Fix case where resource_name not set in stream error ([#746](https://github.com/TraceMachina/nativelink/issues/746)) - ([a651f2c](https://github.com/TraceMachina/nativelink/commit/a651f2ce25238c48c5946d84105d7214fab763ce))
- Set `rust-version` ([#734](https://github.com/TraceMachina/nativelink/issues/734)) - ([d2dd46d](https://github.com/TraceMachina/nativelink/commit/d2dd46da3ae107b2902ca772b084c7231d0d71c3))
- Account for block size in filesystem store for eviction purposes ([#661](https://github.com/TraceMachina/nativelink/issues/661)) - ([0639a59](https://github.com/TraceMachina/nativelink/commit/0639a5973b9bc4fb81e5d53668f43de508aa2b35))
- Fix cargo install tag and start command ([#654](https://github.com/TraceMachina/nativelink/issues/654)) - ([89313ff](https://github.com/TraceMachina/nativelink/commit/89313ff5e1b85e28760d4988a43eb4cfe7b0c848))
- Don't retry permanent failures ([#634](https://github.com/TraceMachina/nativelink/issues/634)) - ([81b64f7](https://github.com/TraceMachina/nativelink/commit/81b64f73e207ad0ae2d87f531f9e93657b11ffd1))
- Reenable caching for nix workflows ([#631](https://github.com/TraceMachina/nativelink/issues/631)) - ([6de799d](https://github.com/TraceMachina/nativelink/commit/6de799dfe5d3d62125c601ce795010cad30b4064))
- Fix AMI NativeLink Tarballing ([#645](https://github.com/TraceMachina/nativelink/issues/645)) - ([c8473ac](https://github.com/TraceMachina/nativelink/commit/c8473ac8a5550afbadc0610804aad30ad82c83a4))
- Evict on touch failure ([#613](https://github.com/TraceMachina/nativelink/issues/613)) - ([3037a66](https://github.com/TraceMachina/nativelink/commit/3037a6625ac98b1e46a70c61ad6160c9a7668809))
- Disable flaky caching for LRE-Remote workflow ([#619](https://github.com/TraceMachina/nativelink/issues/619)) - ([2899f31](https://github.com/TraceMachina/nativelink/commit/2899f31094a58a337521630ac4efaf6276d6e56e))
- Unbreak manual rustfmt invocations via Bazel ([#617](https://github.com/TraceMachina/nativelink/issues/617)) - ([f39e275](https://github.com/TraceMachina/nativelink/commit/f39e2759db044d50224f274f63faac26cb7f931a))
- Fix case where filesystem store future dropping causes issues ([#496](https://github.com/TraceMachina/nativelink/issues/496)) - ([249322d](https://github.com/TraceMachina/nativelink/commit/249322d8436f983c42c8c5da9741119f7609744f))
- Minor refactor of functionally same code ([#607](https://github.com/TraceMachina/nativelink/issues/607)) - ([51715bd](https://github.com/TraceMachina/nativelink/commit/51715bd236f46068da9c94422d9a899dcd14cd18))
- Fix a potential bug in DropCloserReadHalf::take() ([#606](https://github.com/TraceMachina/nativelink/issues/606)) - ([70e8525](https://github.com/TraceMachina/nativelink/commit/70e852598580e48d54835b6ea7d2be6ec953b7b3))
- Fix dark mode accessibility contrast and made theme dynamic based on user machine ([#597](https://github.com/TraceMachina/nativelink/issues/597)) - ([d5443c8](https://github.com/TraceMachina/nativelink/commit/d5443c85aab894d31393215d5d33f6111f3a94cc))

### 📚 Documentation

- Update README.md to include License and Slack ([#841](https://github.com/TraceMachina/nativelink/issues/841)) - ([6c4fb7e](https://github.com/TraceMachina/nativelink/commit/6c4fb7e5577ca5041cb51963457106e6c078c85b))
- Example of chromium using deployment scripts ([#786](https://github.com/TraceMachina/nativelink/issues/786)) - ([0aa7f65](https://github.com/TraceMachina/nativelink/commit/0aa7f65c5a037e3ae3f7b5b79ed285d593b2f214))
- Update README for more clarity ([#803](https://github.com/TraceMachina/nativelink/issues/803)) - ([31a1bf1](https://github.com/TraceMachina/nativelink/commit/31a1bf1e2e7c8ba73624bc998e20c2d551195866))
- Fix incorrect bazel version 6.4.0+ in documenation ([#801](https://github.com/TraceMachina/nativelink/issues/801)) - ([b1b3bcb](https://github.com/TraceMachina/nativelink/commit/b1b3bcb3d5713778d60ecb13afd151b5f50d0209))
- Update js dependencies in docs ([#766](https://github.com/TraceMachina/nativelink/issues/766)) - ([4b8eeaf](https://github.com/TraceMachina/nativelink/commit/4b8eeaf8e3183a66cb68c223fbc22cac66e1f4f6))
- Add search functionality to docs ([#740](https://github.com/TraceMachina/nativelink/issues/740)) - ([3dc1b8e](https://github.com/TraceMachina/nativelink/commit/3dc1b8ece32498b65e68bc270704f2efa902ef1a))
- Add configuration breakdown page ([#725](https://github.com/TraceMachina/nativelink/issues/725)) - ([35daf43](https://github.com/TraceMachina/nativelink/commit/35daf433f01150cdf3b5da4e9a97e561be03cbdf))
- Starts a Breakdown of Configuration ([#680](https://github.com/TraceMachina/nativelink/issues/680)) - ([433829c](https://github.com/TraceMachina/nativelink/commit/433829c961681b7d6bc8ba77384f200def12ba5e))
- Draw a General Purpose Diagram ([#705](https://github.com/TraceMachina/nativelink/issues/705)) - ([2c102c3](https://github.com/TraceMachina/nativelink/commit/2c102c35a082bc935753b25f0df02f8cf47978b9))
- Basic config updated. ([#669](https://github.com/TraceMachina/nativelink/issues/669)) - ([f4d9db3](https://github.com/TraceMachina/nativelink/commit/f4d9db3c12eb75495f642e7d176a7d078d0de193))
- Introduce Vale to lint documentation ([#585](https://github.com/TraceMachina/nativelink/issues/585)) - ([745b0d6](https://github.com/TraceMachina/nativelink/commit/745b0d630d32dd0240aab401dffa3eda09b88305))
- Re-Add Rustup to the README ([#648](https://github.com/TraceMachina/nativelink/issues/648)) - ([0cba4fa](https://github.com/TraceMachina/nativelink/commit/0cba4fa80f7583c7462c157ff60189501ab00658))
- Improve the LRE README ([#637](https://github.com/TraceMachina/nativelink/issues/637)) - ([63826f2](https://github.com/TraceMachina/nativelink/commit/63826f2ea47ba881c7ff05c5eb70b07cff0256e5))
- Update README.md for AWS Terraform Deployment ([#608](https://github.com/TraceMachina/nativelink/issues/608)) - ([8a43fe4](https://github.com/TraceMachina/nativelink/commit/8a43fe4ab2b29a9849e6b69429e2542360118a15))
- Add artifact warning to documentation and swap out cargo emoji ([#599](https://github.com/TraceMachina/nativelink/issues/599)) - ([89eafed](https://github.com/TraceMachina/nativelink/commit/89eafed5aa7d5f6b2bf4bcd7972c963452ba9722))
- Add Kubernetes Example to docs ([#596](https://github.com/TraceMachina/nativelink/issues/596)) - ([e1246fb](https://github.com/TraceMachina/nativelink/commit/e1246fb7f79fd86d1ae0dd0522724bc19ed953b7))
- Fix the bazel run command documentation ([#590](https://github.com/TraceMachina/nativelink/issues/590)) - ([7f4a007](https://github.com/TraceMachina/nativelink/commit/7f4a007f9b5ed24d063a2fcb705816141643f378))
- Add deployment examples to docs ([#584](https://github.com/TraceMachina/nativelink/issues/584)) - ([546484b](https://github.com/TraceMachina/nativelink/commit/546484b86cf9c6c0f1343e68ecf12e9e4e8c5c2d))
- Update README.md ([#580](https://github.com/TraceMachina/nativelink/issues/580)) - ([0269835](https://github.com/TraceMachina/nativelink/commit/0269835f84e550943754cc5d2aa685c21dae05ef))
- Add OSFamily property in basic_cas.json ([#577](https://github.com/TraceMachina/nativelink/issues/577)) - ([3578d50](https://github.com/TraceMachina/nativelink/commit/3578d50fa78387670b7d3761396e4c26b7ee8814))
- Rearrange docs and aligned content with README ([#571](https://github.com/TraceMachina/nativelink/issues/571)) - ([beb87cf](https://github.com/TraceMachina/nativelink/commit/beb87cf91b50c3574b75819e44beb6aa3d96da42))

### 🧪 Testing & CI

- Globally inline format args ([#798](https://github.com/TraceMachina/nativelink/issues/798)) - ([b940f65](https://github.com/TraceMachina/nativelink/commit/b940f65a0bf79ca7a4303a6fed9fba7bc984a9ef))
- Publish nativelink-worker image for C++ ([#794](https://github.com/TraceMachina/nativelink/issues/794)) - ([646253d](https://github.com/TraceMachina/nativelink/commit/646253dec285868263ce77b60c26c9e69daaf1ae))
- Forbid binary files in commits ([#792](https://github.com/TraceMachina/nativelink/issues/792)) - ([d9fc4ad](https://github.com/TraceMachina/nativelink/commit/d9fc4adf71f6680846c7ebd9c2878d02a8aad185))
- Unbreak CI ([#769](https://github.com/TraceMachina/nativelink/issues/769)) - ([682c4fe](https://github.com/TraceMachina/nativelink/commit/682c4feee39b72eb34338e6148c580359a343afc))
- Migrate Bazelisk actions to new variant ([#760](https://github.com/TraceMachina/nativelink/issues/760)) - ([3da42f2](https://github.com/TraceMachina/nativelink/commit/3da42f23badb78428d9868a24468bcbf00f069a7))
- Add hadolint to pre-commit hooks ([#422](https://github.com/TraceMachina/nativelink/issues/422)) - ([d8afd33](https://github.com/TraceMachina/nativelink/commit/d8afd332db15edbf4ee3078a44397b28f6beb529))
- Reduce CI space requirements ([#685](https://github.com/TraceMachina/nativelink/issues/685)) - ([b9029bb](https://github.com/TraceMachina/nativelink/commit/b9029bb073a2d56d1a2b713fdb7d6ff4de69ff64))
- Separate K8s setup steps in CI ([#614](https://github.com/TraceMachina/nativelink/issues/614)) - ([82d9ee6](https://github.com/TraceMachina/nativelink/commit/82d9ee6508df807f284b1a0faf6f22b29ee534e3))

### ⚙️ Miscellaneous

- Generalize Kubernetes worker setup ([#812](https://github.com/TraceMachina/nativelink/issues/812)) - ([4146a34](https://github.com/TraceMachina/nativelink/commit/4146a341a7c0bc31a74296fcb06550f05163eceb))
-  Unify RunningAction and AwaitedAction ([#782](https://github.com/TraceMachina/nativelink/issues/782)) - ([7997f03](https://github.com/TraceMachina/nativelink/commit/7997f03a9426c2778863fea35e585bd752ab6930))
- Don't update rustup in native Cargo workflow ([#775](https://github.com/TraceMachina/nativelink/issues/775)) - ([9d49514](https://github.com/TraceMachina/nativelink/commit/9d4951498547f6550ee71d47e0f9609a463993ee))
- Ignore .direnv for bazel builds ([#756](https://github.com/TraceMachina/nativelink/issues/756)) - ([a15bdb6](https://github.com/TraceMachina/nativelink/commit/a15bdb679a2149a1637d5d1f13d97b2b80587124))
- Set max line length to Rust's defaults ([#750](https://github.com/TraceMachina/nativelink/issues/750)) - ([a876cce](https://github.com/TraceMachina/nativelink/commit/a876ccea65317b512808788c1e26590f3f3b3f02))
- Refactor fs.rs to use call_with_permit scheme ([#741](https://github.com/TraceMachina/nativelink/issues/741)) - ([011318a](https://github.com/TraceMachina/nativelink/commit/011318a7af82d6dcb1d6ffb34af38b159513820c))
- Improve the error message in resource info parsing failure ([#742](https://github.com/TraceMachina/nativelink/issues/742)) - ([3e6f154](https://github.com/TraceMachina/nativelink/commit/3e6f154471e70d37244a66849b1c94a00c1f313f))
- Cleanup hash functions to be more idomatic ([#691](https://github.com/TraceMachina/nativelink/issues/691)) - ([8dd786a](https://github.com/TraceMachina/nativelink/commit/8dd786aca82706145e3d7f32dc2250ddb41e69a9))
- Rename missing `turbo-cache` to `nativelink` ([#663](https://github.com/TraceMachina/nativelink/issues/663)) - ([f8044e6](https://github.com/TraceMachina/nativelink/commit/f8044e66959c52d3cfca840f178f73329e872869))
- Autogenerate version from Cargo.toml ([#660](https://github.com/TraceMachina/nativelink/issues/660)) - ([59d3d28](https://github.com/TraceMachina/nativelink/commit/59d3d284a1f5ed447af25b8fc24ce76a36e6df6a))
- Adjust all instances of Native Link in comments and metadata to NativeLink ([#658](https://github.com/TraceMachina/nativelink/issues/658)) - ([4e7d68b](https://github.com/TraceMachina/nativelink/commit/4e7d68bb1ed6fe8daef9f40ea378a43ac16af956))
- Remove Alpha notice ([#657](https://github.com/TraceMachina/nativelink/issues/657)) - ([a9526b1](https://github.com/TraceMachina/nativelink/commit/a9526b1764e958a947c1b80481419f9d98ff6e26))
- GrpcStore Write Retry ([#638](https://github.com/TraceMachina/nativelink/issues/638)) - ([9f7f45d](https://github.com/TraceMachina/nativelink/commit/9f7f45d626d1f8e9844d4d177250b5274e2bd85d))
- Create workflow for syncing Notion and Issues ([#642](https://github.com/TraceMachina/nativelink/issues/642)) - ([5470857](https://github.com/TraceMachina/nativelink/commit/54708570c32dcf15acbdfcac77084e68ef860c7a))
- Ignore fast store ([#633](https://github.com/TraceMachina/nativelink/issues/633)) - ([f9f7908](https://github.com/TraceMachina/nativelink/commit/f9f79085ac279327428cedda0921aca517c30a7f))
- Migrate to Bzlmod ([#626](https://github.com/TraceMachina/nativelink/issues/626)) - ([2a89ce6](https://github.com/TraceMachina/nativelink/commit/2a89ce6384b428869e21219af303c753bd3087b5))
- Don't cache sanitizer workflows ([#630](https://github.com/TraceMachina/nativelink/issues/630)) - ([ae92fb3](https://github.com/TraceMachina/nativelink/commit/ae92fb30ea00f185118bc11209d53085c70830b8))
- GrpcStore retry first ([#616](https://github.com/TraceMachina/nativelink/issues/616)) - ([30887a9](https://github.com/TraceMachina/nativelink/commit/30887a955f0d1088dddd823d881c197be7ddaf23))
- Helpful Error Output for Integration Test ([#625](https://github.com/TraceMachina/nativelink/issues/625)) - ([39c6678](https://github.com/TraceMachina/nativelink/commit/39c66781284869d284e4e7168a52b387e2e5f2ae))
- Enable blake3 for Bazel builds ([#565](https://github.com/TraceMachina/nativelink/issues/565)) - ([5744813](https://github.com/TraceMachina/nativelink/commit/57448134b24e2a73e02342af05871e0d40a250a9))
- Migrate Mintlify to Docusaurus ([#586](https://github.com/TraceMachina/nativelink/issues/586)) - ([7247385](https://github.com/TraceMachina/nativelink/commit/7247385e9508418f56a5b3a9d3035423484c5830))

### ⬆️ Bumps & Version Updates

- Bump Rust toolchains ([#837](https://github.com/TraceMachina/nativelink/issues/837)) - ([d501cd0](https://github.com/TraceMachina/nativelink/commit/d501cd07a0cb5f8bc34dffaec5649e8070ec8190))
- Update Rust crate prost to 0.12.4 ([#836](https://github.com/TraceMachina/nativelink/issues/836)) - ([8bf14b6](https://github.com/TraceMachina/nativelink/commit/8bf14b621b37f8fdc895cc4526afb25e77151f9f))
- Update h2 to 0.3.26 ([#835](https://github.com/TraceMachina/nativelink/issues/835)) - ([e3913e7](https://github.com/TraceMachina/nativelink/commit/e3913e7b8ac2d88236a2ae6d09756d98c27c18e7))
- Update Rust crate aws-smithy-runtime to 1.2.1 ([#832](https://github.com/TraceMachina/nativelink/issues/832)) - ([77fe4a8](https://github.com/TraceMachina/nativelink/commit/77fe4a86f7366398fbb40a53e67b73e1cec91593))
- Bump express ([#833](https://github.com/TraceMachina/nativelink/issues/833)) - ([2ae7cab](https://github.com/TraceMachina/nativelink/commit/2ae7cab4c7d6cc476bb5de31ffbaf6f59406ce8a))
- Update docusaurus monorepo to v3.2.1 ([#821](https://github.com/TraceMachina/nativelink/issues/821)) - ([d640321](https://github.com/TraceMachina/nativelink/commit/d640321138d7b7e1473347181d29a7fd70068e1e))
- Update docker workflows ([#829](https://github.com/TraceMachina/nativelink/issues/829)) - ([9a3b330](https://github.com/TraceMachina/nativelink/commit/9a3b330a86c2b78fe19ecdac740bd8e72241bf95))
- Update nix environment ([#830](https://github.com/TraceMachina/nativelink/issues/830)) - ([6b9e68e](https://github.com/TraceMachina/nativelink/commit/6b9e68effc6d5d19118f5cead6ea036c97dea609))
- Update Configuration.mdx ([#822](https://github.com/TraceMachina/nativelink/issues/822)) - ([15b455c](https://github.com/TraceMachina/nativelink/commit/15b455c1d7797dcf575aaa57e10e0736cd409877))
- Update Rust crate lz4_flex to 0.11.3 ([#820](https://github.com/TraceMachina/nativelink/issues/820)) - ([5a3a37d](https://github.com/TraceMachina/nativelink/commit/5a3a37d828474ed84d214daf6945ad14fc4f04e0))
- Update Rust crate pin-project-lite to 0.2.14 ([#818](https://github.com/TraceMachina/nativelink/issues/818)) - ([75f98e8](https://github.com/TraceMachina/nativelink/commit/75f98e8e9e2a52f7dbba5c7351e4ebb2b561708c))
- Update Rust crate tokio to 1.37.0 ([#813](https://github.com/TraceMachina/nativelink/issues/813)) - ([9e00ebb](https://github.com/TraceMachina/nativelink/commit/9e00ebb19112b507c0a5fb8b86156f6e30dcef34))
- Update Rust crate aws-sdk-s3 to 1.21.0 ([#802](https://github.com/TraceMachina/nativelink/issues/802)) - ([1dd302d](https://github.com/TraceMachina/nativelink/commit/1dd302d9442e36e105a705c388b8a1514b1f692c))
- Update node dependencies ([#805](https://github.com/TraceMachina/nativelink/issues/805)) - ([b6d4427](https://github.com/TraceMachina/nativelink/commit/b6d4427547f35d24763cbd921de3eab28e738e7c))
- Update Rust crate clap to 4.5.4 ([#799](https://github.com/TraceMachina/nativelink/issues/799)) - ([00ff4a0](https://github.com/TraceMachina/nativelink/commit/00ff4a088365e616e6094c85d99d999a039338b8))
- Update Rust crate aws-config to 1.1.9 ([#796](https://github.com/TraceMachina/nativelink/issues/796)) - ([f601cd0](https://github.com/TraceMachina/nativelink/commit/f601cd079cc866854056faa2788659c0014e2d4e))
- Update Rust crate async-trait to 0.1.79 ([#790](https://github.com/TraceMachina/nativelink/issues/790)) - ([09defc6](https://github.com/TraceMachina/nativelink/commit/09defc6737da5034e6e102f44d68ab1edbc25265))
- Update Rust crate bytes to 1.6.0 ([#787](https://github.com/TraceMachina/nativelink/issues/787)) - ([08539ec](https://github.com/TraceMachina/nativelink/commit/08539ecb810232100b871754556a9b328e86b501))
- Update dependency platforms to v0.0.9 ([#784](https://github.com/TraceMachina/nativelink/issues/784)) - ([a6976e0](https://github.com/TraceMachina/nativelink/commit/a6976e095403dfd7cf03c554c8ce681af40622e5))
- Update dependency rules_java to v7.5.0 ([#780](https://github.com/TraceMachina/nativelink/issues/780)) - ([a6d0f64](https://github.com/TraceMachina/nativelink/commit/a6d0f64c219eb007ae32468d1a3d5915ec3f869c))
- Update Rust crate uuid to 1.8.0 ([#776](https://github.com/TraceMachina/nativelink/issues/776)) - ([4095e97](https://github.com/TraceMachina/nativelink/commit/4095e978cf7b0d7e13f25bad80214753220b6ecf))
- Update Rust crate aws-sdk-s3 to 1.20.0 ([#774](https://github.com/TraceMachina/nativelink/issues/774)) - ([d3ee9b6](https://github.com/TraceMachina/nativelink/commit/d3ee9b6c40f7dc8e1faaf91f48713ade6d95da0f))
- Update Rust crate async-trait to 0.1.78 ([#771](https://github.com/TraceMachina/nativelink/issues/771)) - ([2960469](https://github.com/TraceMachina/nativelink/commit/29604699d0475357a23007d4192da4b0f3c78857))
- Update Rust crate aws-sdk-s3 to 1.19.1 ([#767](https://github.com/TraceMachina/nativelink/issues/767)) - ([10d5599](https://github.com/TraceMachina/nativelink/commit/10d559998458f7ca0f74e8bbda3bee861541700d))
- Update flake ([#765](https://github.com/TraceMachina/nativelink/issues/765)) - ([63a01c5](https://github.com/TraceMachina/nativelink/commit/63a01c54c8315ff74681835f6f7d065892b09428))
- Update Rust crate clap to 4.5.3 ([#763](https://github.com/TraceMachina/nativelink/issues/763)) - ([3783abc](https://github.com/TraceMachina/nativelink/commit/3783abcd0e502025b9d8f1fb845e2ba0a1d77d25))
- Update Rust crate aws-sdk-s3 to 1.19.0 ([#762](https://github.com/TraceMachina/nativelink/issues/762)) - ([aa599c3](https://github.com/TraceMachina/nativelink/commit/aa599c30bedfc6e0e67d388517964896cf86a3bc))
- Update Rust crate tokio-stream to 0.1.15 ([#761](https://github.com/TraceMachina/nativelink/issues/761)) - ([d8b514c](https://github.com/TraceMachina/nativelink/commit/d8b514cd0264ff33c3cccde68cd6dc2e69f61b1a))
- Update aws-sdk-rust monorepo ([#759](https://github.com/TraceMachina/nativelink/issues/759)) - ([4dc541e](https://github.com/TraceMachina/nativelink/commit/4dc541e7ccf21575522f98a7e5e4c12f16ad1560))
- Update Rust crate blake3 to 1.5.1 ([#758](https://github.com/TraceMachina/nativelink/issues/758)) - ([d6e6863](https://github.com/TraceMachina/nativelink/commit/d6e6863b2dcbe2c34e78fa4168a706ca34608d29))
- Update TypeScript dependencies ([#753](https://github.com/TraceMachina/nativelink/issues/753)) - ([4163da1](https://github.com/TraceMachina/nativelink/commit/4163da1fb0277ad23becf52514ae9ee8271a7fa4))
- Update Rust crate clap to 4.5.2 ([#754](https://github.com/TraceMachina/nativelink/issues/754)) - ([d3fa8b2](https://github.com/TraceMachina/nativelink/commit/d3fa8b2ca4491e8638b7e5ffd288dbb94bfbe0fb))
- Update Rust crate http to 1.1.0 ([#549](https://github.com/TraceMachina/nativelink/issues/549)) - ([14a4493](https://github.com/TraceMachina/nativelink/commit/14a44937704b92ba9997c719e7568217ab97f38f))
- Optimize hashing files ([#720](https://github.com/TraceMachina/nativelink/issues/720)) - ([0fa9a40](https://github.com/TraceMachina/nativelink/commit/0fa9a409e21dee8a67f2f688a1577ba0e4d83d8f))
- Bump mio to v0.8.11 ([#719](https://github.com/TraceMachina/nativelink/issues/719)) - ([7169fc9](https://github.com/TraceMachina/nativelink/commit/7169fc9ccd0248330841532f66a263e505d35529))
- Update step-security/harden-runner action to v2.7.0 ([#718](https://github.com/TraceMachina/nativelink/issues/718)) - ([44cb709](https://github.com/TraceMachina/nativelink/commit/44cb709aabd4e2f5ae3fdf7c552039c233089a97))
- Update dependency rules_java to v7.4.0 ([#715](https://github.com/TraceMachina/nativelink/issues/715)) - ([6058d6a](https://github.com/TraceMachina/nativelink/commit/6058d6a80eefe06e83acd5e8f601201390f4a7b8))
- Update Rust crate uuid to 1.7.0 ([#711](https://github.com/TraceMachina/nativelink/issues/711)) - ([fdf232c](https://github.com/TraceMachina/nativelink/commit/fdf232c6d4fa168dbc66540adcf82a374b439150))
- Update Rust crate tokio to 1.36.0 ([#710](https://github.com/TraceMachina/nativelink/issues/710)) - ([058828f](https://github.com/TraceMachina/nativelink/commit/058828f91b7959a7dac83e4ba8111a08996732e1))
- Update Rust crate tempfile to 3.10.1 ([#709](https://github.com/TraceMachina/nativelink/issues/709)) - ([aa79732](https://github.com/TraceMachina/nativelink/commit/aa7973225854414e7709c926bfa394d05f3ddcae))
- Update Rust crate shlex to 1.3.0 ([#707](https://github.com/TraceMachina/nativelink/issues/707)) - ([bd8d31a](https://github.com/TraceMachina/nativelink/commit/bd8d31a3667e6e4678fe30b2ddfa70caf98084cf))
- Update Rust crate serde to 1.0.197 ([#706](https://github.com/TraceMachina/nativelink/issues/706)) - ([fb761b7](https://github.com/TraceMachina/nativelink/commit/fb761b703e916956859eb7c80b99f71e95f69d5a))
- Update Rust crate rustls-pemfile to 2.1.1 ([#704](https://github.com/TraceMachina/nativelink/issues/704)) - ([59c2dd0](https://github.com/TraceMachina/nativelink/commit/59c2dd0cc0843d9ec1f169fc52369700227d9198))
- Update Rust crate relative-path to 1.9.2 ([#703](https://github.com/TraceMachina/nativelink/issues/703)) - ([e6ae832](https://github.com/TraceMachina/nativelink/commit/e6ae832b93938f87e3198bc61cdea9cc0ef1d77f))
- Update Rust crate lz4_flex to 0.11.2 ([#701](https://github.com/TraceMachina/nativelink/issues/701)) - ([1840ca8](https://github.com/TraceMachina/nativelink/commit/1840ca879a01e039c437d1ff7ada749aaf330c6d))
- Update Rust crate mock_instant to 0.3.2 ([#702](https://github.com/TraceMachina/nativelink/issues/702)) - ([ae0ba19](https://github.com/TraceMachina/nativelink/commit/ae0ba1962dc5b58dd1a94aafbb81012733904392))
- Update Rust crate clap to 4.5.1 ([#698](https://github.com/TraceMachina/nativelink/issues/698)) - ([5427781](https://github.com/TraceMachina/nativelink/commit/5427781feef001e6116bcdebbea0dfb31fa9ebea))
- Update Rust crate lru to 0.12.3 ([#700](https://github.com/TraceMachina/nativelink/issues/700)) - ([37184e8](https://github.com/TraceMachina/nativelink/commit/37184e887b0b3f0812bb4553eb3a9d30a773c419))
- Update Rust crate log to 0.4.21 ([#699](https://github.com/TraceMachina/nativelink/issues/699)) - ([6364ddf](https://github.com/TraceMachina/nativelink/commit/6364ddf1a0d6ee3cb2896798f6b52cdda9d257ca))
- Update Rust crate async-trait to 0.1.77 ([#695](https://github.com/TraceMachina/nativelink/issues/695)) - ([34af738](https://github.com/TraceMachina/nativelink/commit/34af7382f0167ace594129c209bdd14d4ffd0d25))
- Update Rust crate futures to 0.3.30 ([#697](https://github.com/TraceMachina/nativelink/issues/697)) - ([ab21dc5](https://github.com/TraceMachina/nativelink/commit/ab21dc5e799211847e0319864e4502c861e6f522))
- Update AWS SDK to 1.x ([#684](https://github.com/TraceMachina/nativelink/issues/684)) - ([cd78ed2](https://github.com/TraceMachina/nativelink/commit/cd78ed27446f7324c5f6301935223b255f2b90bb))
- Update Bazel-tracked toolchains ([#690](https://github.com/TraceMachina/nativelink/issues/690)) - ([c5851f9](https://github.com/TraceMachina/nativelink/commit/c5851f9b8ac41fc31438b713912d1760bf6fe657))
- Update GHA workflows ([#696](https://github.com/TraceMachina/nativelink/issues/696)) - ([b0fcac8](https://github.com/TraceMachina/nativelink/commit/b0fcac80a6116eca3bc1aa322abc4bafb20483c5))
- Update Rust crate async-lock to 3.3.0 ([#693](https://github.com/TraceMachina/nativelink/issues/693)) - ([65f89aa](https://github.com/TraceMachina/nativelink/commit/65f89aaa243b0b8eb6c842a1c85a6a0fc7f95653))
- Bump development environment ([#686](https://github.com/TraceMachina/nativelink/issues/686)) - ([0fd8b51](https://github.com/TraceMachina/nativelink/commit/0fd8b51a6f4106ef0ba466e2c677e3a2fb7fdb6b))
- Update Rust crate hyper to 0.14.28 ([#531](https://github.com/TraceMachina/nativelink/issues/531)) - ([6491fc7](https://github.com/TraceMachina/nativelink/commit/6491fc76f5ea3ec8b6a70694694afdfae92f72fa))
- [Security] Bump trivially bumpable deps ([#629](https://github.com/TraceMachina/nativelink/issues/629)) - ([20887ac](https://github.com/TraceMachina/nativelink/commit/20887acc296f3da2363607b12c78c54ace94bd95))
- EvictingMap should evict keys on all public access. ([#601](https://github.com/TraceMachina/nativelink/issues/601)) - ([56a0972](https://github.com/TraceMachina/nativelink/commit/56a0972402cb8ec5df04da8ee4cd307ed3650f28))
- Update rules_rust to 0.36.2 ([#588](https://github.com/TraceMachina/nativelink/issues/588)) - ([4cfadb3](https://github.com/TraceMachina/nativelink/commit/4cfadb3fc764ff61719e517ff0e3a1272efd5eab))

## [0.2.0](https://github.com/TraceMachina/nativelink/compare/v0.1.0..v0.2.0) - 2023-12-21



### ❌️  Breaking Changes

- [Breaking] Rename cas executable to nativelink ([#573](https://github.com/TraceMachina/nativelink/issues/573)) - ([ddf1d74](https://github.com/TraceMachina/nativelink/commit/ddf1d74ba952a825e88bc68ed1efd67c6386d190))

### 📚 Documentation

- Reorder README for Simplicity ([#563](https://github.com/TraceMachina/nativelink/issues/563)) - ([b12dfb8](https://github.com/TraceMachina/nativelink/commit/b12dfb843a0702f42f888d4babfb4f909ba8381f))

### 🧪 Testing & CI

- Add Nix formatters and linters to pre-commit hooks ([#561](https://github.com/TraceMachina/nativelink/issues/561)) - ([d823964](https://github.com/TraceMachina/nativelink/commit/d8239640a9fa26c932a4c234ee2d263837159388))
- Fix kill_all_waits_for_all_tasks_to_finish test stuck on windows ([#525](https://github.com/TraceMachina/nativelink/issues/525)) - ([143a5a1](https://github.com/TraceMachina/nativelink/commit/143a5a178028c3d94e4623a67eef8a2d58e7cca7))
- Fix missing timeouts in tests ([#553](https://github.com/TraceMachina/nativelink/issues/553)) - ([c54c51c](https://github.com/TraceMachina/nativelink/commit/c54c51cf91847e48e84cf75a69a2531fc4478776))
- Remove many of the large-* images in CI ([#552](https://github.com/TraceMachina/nativelink/issues/552)) - ([de0ae1e](https://github.com/TraceMachina/nativelink/commit/de0ae1eaa92155ab45b69cf61fa48c221ee78a42))

### ⚙️ Miscellaneous

- Publish SemVer-tagged images on tag pushes to main ([#569](https://github.com/TraceMachina/nativelink/issues/569)) - ([758c5d7](https://github.com/TraceMachina/nativelink/commit/758c5d7268a2cacf7dc3ae11f2b0f83007d6b6bb))
- S3 Store credential provider ([#494](https://github.com/TraceMachina/nativelink/issues/494)) - ([1039ea0](https://github.com/TraceMachina/nativelink/commit/1039ea044ddeacc21361841751eb7ba29651178c))
- fix a typo ([#560](https://github.com/TraceMachina/nativelink/issues/560)) - ([ff6d097](https://github.com/TraceMachina/nativelink/commit/ff6d0975666588d1373bcc6e315f24c4a30a0786))

### ⬆️ Bumps & Version Updates

- Update Rust crate async-lock to v3 ([#548](https://github.com/TraceMachina/nativelink/issues/548)) - ([6c555bb](https://github.com/TraceMachina/nativelink/commit/6c555bb4e777af1563219102a34571ce02178c89))
- Update OSSF domain ([#558](https://github.com/TraceMachina/nativelink/issues/558)) - ([82603d2](https://github.com/TraceMachina/nativelink/commit/82603d23f01df3cd26bf8005001df35de6f050b7))
- Update LLVM and rust toolchains ([#557](https://github.com/TraceMachina/nativelink/issues/557)) - ([1726a1a](https://github.com/TraceMachina/nativelink/commit/1726a1af0e3e3fd61373b1c791a5993f94590024))
- Update actions/checkout action to v4 ([#556](https://github.com/TraceMachina/nativelink/issues/556)) - ([0d18d36](https://github.com/TraceMachina/nativelink/commit/0d18d36c572db73db00c6e4b22d436d7bc5983af))
- Update Rust crate tokio to 1.35.1 ([#535](https://github.com/TraceMachina/nativelink/issues/535)) - ([c6f8b8a](https://github.com/TraceMachina/nativelink/commit/c6f8b8ab58e3fbef77a1b4db68b1955557444fd0))
- Update Rust crate tokio-rustls to 0.25.0 & rustls-pemfile to 2.0.0 ([#540](https://github.com/TraceMachina/nativelink/issues/540)) - ([cb76d18](https://github.com/TraceMachina/nativelink/commit/cb76d189d3187a043aed4e29962f6fa1c97616b1))
- Update actions/checkout action to v3.6.0 ([#541](https://github.com/TraceMachina/nativelink/issues/541)) - ([5dce4ce](https://github.com/TraceMachina/nativelink/commit/5dce4ce6f08562a47d8fc0c3d1c2f57d06550ad8))
- Update dependency rules_python to v0.27.1 ([#546](https://github.com/TraceMachina/nativelink/issues/546)) - ([6ef8b6c](https://github.com/TraceMachina/nativelink/commit/6ef8b6cb233acf33de475f9f61129bfe6d90c571))
- Update dependency rules_rust to v0.34.1 ([#547](https://github.com/TraceMachina/nativelink/issues/547)) - ([637f283](https://github.com/TraceMachina/nativelink/commit/637f2834138f86be45c12cf46623de539148fe24))
- Update dependency @google-cloud/compute to v4.1.0 ([#544](https://github.com/TraceMachina/nativelink/issues/544)) - ([dbac23a](https://github.com/TraceMachina/nativelink/commit/dbac23afa27f55c662f8a1d0539cc8fc82717afe))

## [0.1.0](https://github.com/TraceMachina/nativelink/compare/v1.0.1..v0.1.0) - 2023-12-20



### ❌️  Breaking Changes

- [Breaking] Mark S3 store experimental - ([05a6dd7](https://github.com/TraceMachina/nativelink/commit/05a6dd79635a98411d90505ff500694092c2f927))
- [Breaking] listen_address renamed/remapped in config ([#476](https://github.com/TraceMachina/nativelink/issues/476)) - ([9db28d6](https://github.com/TraceMachina/nativelink/commit/9db28d6a33bb3d07224ddf39b9be9a2b8a2afccd))
- [Breaking] Rename entrypoint_cmd->entrypoint and precondition_script ([#475](https://github.com/TraceMachina/nativelink/issues/475)) - ([dbe61d2](https://github.com/TraceMachina/nativelink/commit/dbe61d281520d20dba477ddb430139338afabde6))
- [Breaking] Mark prometheus config as experimental ([#473](https://github.com/TraceMachina/nativelink/issues/473)) - ([931e721](https://github.com/TraceMachina/nativelink/commit/931e72156879f3bba38b888c20ad55b9584991e5))
- [Breaking] Standardize configurations so they are all lower case ([#461](https://github.com/TraceMachina/nativelink/issues/461)) - ([3329d7c](https://github.com/TraceMachina/nativelink/commit/3329d7cd8adf206c4a4d84cd801f4d13c8bb6052))
- [Breaking Change] Message field can now be populated ([#361](https://github.com/TraceMachina/nativelink/issues/361)) - ([cf2f3e4](https://github.com/TraceMachina/nativelink/commit/cf2f3e458a7ae26fb0dc730ff09bfedd437f6216))
- [Breaking Change] Add store type to GrpcStore. - ([e1f3716](https://github.com/TraceMachina/nativelink/commit/e1f37167ed1ae98e313fb8fd5375881bc50b98af))
- [BreakingChange] Scheduler config now supports multiple impls - ([384f14e](https://github.com/TraceMachina/nativelink/commit/384f14e593e88294ffbe01471416b8d1424442ac))

### ⛰️  Features

- Add renovate.json ([#487](https://github.com/TraceMachina/nativelink/issues/487)) - ([933963f](https://github.com/TraceMachina/nativelink/commit/933963f1b207f7d1b4f4cdb0b1ae620de8533336))
- Add OSFamily and container-image platform props ([#512](https://github.com/TraceMachina/nativelink/issues/512)) - ([b6b8252](https://github.com/TraceMachina/nativelink/commit/b6b82528e6db077a1159a6b8472a08cd9537dbe3))
- Add fancy badges ([#521](https://github.com/TraceMachina/nativelink/issues/521)) - ([e122042](https://github.com/TraceMachina/nativelink/commit/e122042d5e38ddebfebb888114092a1227dc8a27))
- Add Git-Cliff Changelog ([#515](https://github.com/TraceMachina/nativelink/issues/515)) - ([8197bb9](https://github.com/TraceMachina/nativelink/commit/8197bb9712a4e470e0cb07a7a460e98054ce5307))
- Integrate google analytics ([#503](https://github.com/TraceMachina/nativelink/issues/503)) - ([ef74f9c](https://github.com/TraceMachina/nativelink/commit/ef74f9c0ca746283a38312f8b0bf5ec9f74d163b))
- Add OpenSSF scorecard action ([#486](https://github.com/TraceMachina/nativelink/issues/486)) - ([4d9d897](https://github.com/TraceMachina/nativelink/commit/4d9d8973313c07e22984622e6bbc1947d2ba7785))
- Add Completeness Checking Store ([#404](https://github.com/TraceMachina/nativelink/issues/404)) - ([d264624](https://github.com/TraceMachina/nativelink/commit/d26462407cdc04b5a4eb4dc4d46b298db996c43f))
- Publish container images ([#443](https://github.com/TraceMachina/nativelink/issues/443)) - ([697cddf](https://github.com/TraceMachina/nativelink/commit/697cddfe0adb1964f469e272d843b76346c1884a))
- Add function to Store API to get the inner store when possible ([#410](https://github.com/TraceMachina/nativelink/issues/410)) - ([a0788fa](https://github.com/TraceMachina/nativelink/commit/a0788fabc1831714e39fa5047e0a385a2c62234f))
- Add GCP to terraform deployment examples ([#433](https://github.com/TraceMachina/nativelink/issues/433)) - ([4661a36](https://github.com/TraceMachina/nativelink/commit/4661a36b40cd89fdf20e5af1c78745e75c60ec74))
- Add Blake3 digest support ([#403](https://github.com/TraceMachina/nativelink/issues/403)) - ([2c8f0f0](https://github.com/TraceMachina/nativelink/commit/2c8f0f0f0a68b3033045ea88cf4cdbf5c968d9d9))
- Add Noop store ([#408](https://github.com/TraceMachina/nativelink/issues/408)) - ([aea3768](https://github.com/TraceMachina/nativelink/commit/aea37682dbed261c401e5025ffd77dff2711f699))
- Add DigestHasher as interface to hashing functions ([#400](https://github.com/TraceMachina/nativelink/issues/400)) - ([9e31ca4](https://github.com/TraceMachina/nativelink/commit/9e31ca463632b2974c86f75f3ff20a4fb93ba3e5))
- Add rustc explicitly to flake ([#398](https://github.com/TraceMachina/nativelink/issues/398)) - ([db724c0](https://github.com/TraceMachina/nativelink/commit/db724c0fc3a21798dd876578507fec5115443233))
- Add existence cache ([#383](https://github.com/TraceMachina/nativelink/issues/383)) - ([e8e6701](https://github.com/TraceMachina/nativelink/commit/e8e670176d225b49148d341109de963ea81c6718))
- Add ability for external scripts (ie: entrypoint_cmd) to manage timeout ([#368](https://github.com/TraceMachina/nativelink/issues/368)) - ([3ae120a](https://github.com/TraceMachina/nativelink/commit/3ae120ac479cde26873cd01d76d3c37cbb05d78c))
- Add Http2 flags for advanced configurations ([#365](https://github.com/TraceMachina/nativelink/issues/365)) - ([cb04ed4](https://github.com/TraceMachina/nativelink/commit/cb04ed48f8977147a03b232414cedc884370cd95))
- Add summary of platform properties to prometheus ([#367](https://github.com/TraceMachina/nativelink/issues/367)) - ([d9af3b9](https://github.com/TraceMachina/nativelink/commit/d9af3b99876f2df9cbc42989d1b06d737d89e387))
- Add more err_tip for easier debugging ([#363](https://github.com/TraceMachina/nativelink/issues/363)) - ([b5ff95d](https://github.com/TraceMachina/nativelink/commit/b5ff95dd9c6f5640d460c4e3c7cea6c0449cbc28))
- Add security policy ([#343](https://github.com/TraceMachina/nativelink/issues/343)) - ([9173c2f](https://github.com/TraceMachina/nativelink/commit/9173c2fcd20b522a5d249fae0044d337b7c2fa9d))
- Add retry to GrpcScheduler ([#324](https://github.com/TraceMachina/nativelink/issues/324)) - ([21519ce](https://github.com/TraceMachina/nativelink/commit/21519ceba07ad81c831d99442c1e17363822fef3))
- Add ability to ignore EOF check for writers ([#341](https://github.com/TraceMachina/nativelink/issues/341)) - ([979f941](https://github.com/TraceMachina/nativelink/commit/979f94133f9d2826ac737211b5e9bcbf11f55cee))
- Introduce Nix development flake ([#330](https://github.com/TraceMachina/nativelink/issues/330)) - ([a0792fd](https://github.com/TraceMachina/nativelink/commit/a0792fdf0560c3324d793d94c84d02dfcd892271))
- Introduce Bazel build for Windows ([#317](https://github.com/TraceMachina/nativelink/issues/317)) - ([659d571](https://github.com/TraceMachina/nativelink/commit/659d571abb4d79c0ad80b542e57978e5ec8331bc))
- Added tracking for all client connections since server started and time server started - ([0375a8f](https://github.com/TraceMachina/nativelink/commit/0375a8f41ad603b2c0b9cf440ca247b85dd4349b))
- Introduced shard store - ([a7e3936](https://github.com/TraceMachina/nativelink/commit/a7e39360c4a63418cfdd350bf50660c6ba126e16))
- Add Contributing file - ([4900f06](https://github.com/TraceMachina/nativelink/commit/4900f06bc1a171e6603a773b2fc89609191611a9))
- Add ADDITIONAL_SETUP_WORKER_CMD to Dockerfile - ([3c30387](https://github.com/TraceMachina/nativelink/commit/3c30387207c1d8bd01e31760127b579c20e626a2))
- Add windows support - ([2875f0b](https://github.com/TraceMachina/nativelink/commit/2875f0b3dd2ddf4076a2186b6212366ea89b6958))
- Add support to build with Cargo - ([bff3be3](https://github.com/TraceMachina/nativelink/commit/bff3be35490842b318b9533f4c517b67b4e2e45d))
- Add metrics to SimpleScheduler and Worker - ([63f7393](https://github.com/TraceMachina/nativelink/commit/63f73936b6f2ba65ede938c1ea50aa7a8a284d4a))
- Add ability for metris to be disabled - ([875b3ca](https://github.com/TraceMachina/nativelink/commit/875b3ca47028ac43fe9d905bbf315f07d4c7b5ae))
- Add property modifying scheduler. - ([656e7f7](https://github.com/TraceMachina/nativelink/commit/656e7f7db00b12443996fa370076a4695e10768f))
- Add metrics to LocalWorker and RunningActionsManager - ([f0a526b](https://github.com/TraceMachina/nativelink/commit/f0a526b400b8f159d7d1005a9907cfad913f6226))
- Add prometheus stats to MemoryStore - ([f274dcf](https://github.com/TraceMachina/nativelink/commit/f274dcf32b1b57153ad95f75af8fbe61a7410975))
- Add retry to GrpcStore. - ([259224b](https://github.com/TraceMachina/nativelink/commit/259224b28ec8b2f9d878bf079ddaea679baf082a))
- Add prometheus stats for VerifyStore - ([5f5b2c4](https://github.com/TraceMachina/nativelink/commit/5f5b2c487fa800c0aa519a74f6bd3e7c12f1d795))
- Add prometheus publishing and hook up FilesystemStore - ([04a7772](https://github.com/TraceMachina/nativelink/commit/04a77724b353bc86a381b62d33a0621e7c11b52f))
- Add support for backpressure from workers. - ([fc97fcb](https://github.com/TraceMachina/nativelink/commit/fc97fcb1f85131997a9db7068134973116486f6a))
- Add ability to create low watermark to avoid thrashing against eviction cap. - ([e16b45c](https://github.com/TraceMachina/nativelink/commit/e16b45c155b697f0f4be9af5004437afa0a016fd))
- Add is_empty to LenEntry - ([e643090](https://github.com/TraceMachina/nativelink/commit/e6430900ef21ad4bc651eb0076060b513ca8c3b3))
- Add timestamps to executor jobs. - ([fa97b28](https://github.com/TraceMachina/nativelink/commit/fa97b288bb683e78e95b5805883da632396b4034))

### 🐛 Bug Fixes

- Remove Fixed-Buffer Dependency ([#509](https://github.com/TraceMachina/nativelink/issues/509)) - ([5a6b182](https://github.com/TraceMachina/nativelink/commit/5a6b182c13e006119d858b5fab759d17938b0c65))
- Fix rustfmt after 6d07a86 ([#520](https://github.com/TraceMachina/nativelink/issues/520)) - ([cfdf7e8](https://github.com/TraceMachina/nativelink/commit/cfdf7e8a1ee173e5b303cf0d61b1d4adf08d38bd))
- Fixes error forwarding to client for failed command executions ([#432](https://github.com/TraceMachina/nativelink/issues/432)) - ([0c225da](https://github.com/TraceMachina/nativelink/commit/0c225da70bd4ad23ed359e1b86efe2009af3df55))
- Fix unwrap function in the Prometheus server code ([#446](https://github.com/TraceMachina/nativelink/issues/446)) - ([406eab7](https://github.com/TraceMachina/nativelink/commit/406eab7d664167e2eadbd49754fd3ecc0b2f3a56))
- Refactor filesystem store for timeout function passing ([#439](https://github.com/TraceMachina/nativelink/issues/439)) - ([5123ffc](https://github.com/TraceMachina/nativelink/commit/5123ffcb3ed10f8b951a2a99edce50bcaa02f49e))
- Handle SIGINT ([#434](https://github.com/TraceMachina/nativelink/issues/434)) - ([f9e537c](https://github.com/TraceMachina/nativelink/commit/f9e537c3f9b5656be6251902640ff003a5b8cc48))
- Fixup configs to have defaults & digest function uses lower case ([#438](https://github.com/TraceMachina/nativelink/issues/438)) - ([d56f008](https://github.com/TraceMachina/nativelink/commit/d56f008c05ab120d039c6db6bef145446cec97ff))
- Fix AWS terraform deployment ([#423](https://github.com/TraceMachina/nativelink/issues/423)) - ([4cc53bc](https://github.com/TraceMachina/nativelink/commit/4cc53bc82286cce57854f6e7c2765f03932ac370))
- Fix empty bytes error in s3 store and support AWS_ENDPOINT_URL ([#421](https://github.com/TraceMachina/nativelink/issues/421)) - ([cf531dc](https://github.com/TraceMachina/nativelink/commit/cf531dc6e2d3fc7038e73ed5a0848a8c5c3a1518))
- Migrate S3 store to official AWS SDK ([#369](https://github.com/TraceMachina/nativelink/issues/369)) - ([6ce11ab](https://github.com/TraceMachina/nativelink/commit/6ce11ab10120b3e3ca65902c2c20c508865b7b45))
- Fix double negative when computing remaining memory % in terraform deployment ([#407](https://github.com/TraceMachina/nativelink/issues/407)) - ([9e981a5](https://github.com/TraceMachina/nativelink/commit/9e981a54cd43dec27d97c99a0ba5d015dab6bec1))
- Fix the typo of WorkerProperty ([#391](https://github.com/TraceMachina/nativelink/issues/391)) - ([8a1cb6b](https://github.com/TraceMachina/nativelink/commit/8a1cb6b610f980de8c90e5db9a6f73de8470c73a))
- Retry GrpcStore write ([#326](https://github.com/TraceMachina/nativelink/issues/326)) - ([6006e23](https://github.com/TraceMachina/nativelink/commit/6006e23b10350cd1a0445f23a6a0b0d6dd5dcf02))
- Revert "Fix never looping loops ([#372](https://github.com/TraceMachina/nativelink/issues/372))" ([#373](https://github.com/TraceMachina/nativelink/issues/373)) - ([8e234c5](https://github.com/TraceMachina/nativelink/commit/8e234c574105ee6821eab7b7d3980f43a69f45e9))
- Fix never looping loops ([#372](https://github.com/TraceMachina/nativelink/issues/372)) - ([755c10e](https://github.com/TraceMachina/nativelink/commit/755c10ef0c33e07a21fef7da692594745723a625))
- Close on complete in GrpcScheduler ([#328](https://github.com/TraceMachina/nativelink/issues/328)) - ([6c937da](https://github.com/TraceMachina/nativelink/commit/6c937da3264dcc6e7cf8d9731db254677c813405))
- Fix potential race condition if worker disconnects - ([b871a90](https://github.com/TraceMachina/nativelink/commit/b871a90573ba9561f95280246d94897bdd4466a8))
- Don't download zero size blobs - ([c8e2ee8](https://github.com/TraceMachina/nativelink/commit/c8e2ee83dcb7e09c20408b2f09371ca261dfb8f3))
- Fix prometheus metrics to not publish multiple times - ([f42f150](https://github.com/TraceMachina/nativelink/commit/f42f150926c23faba7aa63ba62a40eabb1ce8b20))
- Fix readme TLDR - ([b6a4046](https://github.com/TraceMachina/nativelink/commit/b6a404600261815028038de1939314421cb8ff29))
- Fix default config regression in master - ([bca2f3d](https://github.com/TraceMachina/nativelink/commit/bca2f3dfd49bc16e29fec7e6775535838e0d4731))
- Fix fence post bugs in dedup store - ([d7c847c](https://github.com/TraceMachina/nativelink/commit/d7c847c85410047c26ac7361446b27c2e6b3b357))
- Fix the AWS deployment examples - ([17bfbf6](https://github.com/TraceMachina/nativelink/commit/17bfbf670b2aeda504f20e82cd5cd1c39e32792a))
- Fix inefficient upload of stderr/stdout in workers - ([8ac4824](https://github.com/TraceMachina/nativelink/commit/8ac4824d1d58379348b50a52cad331e417d1accf))
- Don't remove Error context. - ([e9ab61e](https://github.com/TraceMachina/nativelink/commit/e9ab61e8d8d204c34e50a3c5ec62d6fb75505aae))
- Fix clippy warnings for scheduler directory - ([1491d0a](https://github.com/TraceMachina/nativelink/commit/1491d0a6878dd17f18944ec4a1b36544aee3d148))
- Fix potential bug where scheduer could drop action - ([f118ccd](https://github.com/TraceMachina/nativelink/commit/f118ccd264e9e68acc2c34474f4024dd7e632f2e))
- Fix "unused function" warnings in utf8_range - ([f048352](https://github.com/TraceMachina/nativelink/commit/f04835203e31b73b8a580b4037b143c80f3567d0))
- Fix digest clones and a few other minor clippy warnings - ([a523115](https://github.com/TraceMachina/nativelink/commit/a5231150ac8a962941f7691138037db4610d636a))
- Fix clippy messages in cas/store - ([7fef931](https://github.com/TraceMachina/nativelink/commit/7fef9312ae62f291c1dc9dd1988b2e888bc6fd03))
- Fix clippy erros for most other non-scheduler files - ([264849b](https://github.com/TraceMachina/nativelink/commit/264849b8aee7bc60d05ee8bb2725b90fc4f3dfbd))
- Fix clippy cas/grpc_service folder - ([e85faed](https://github.com/TraceMachina/nativelink/commit/e85faed862e9911cf1e48d4aa0a0aec361ba19b4))
- Fix most clippy warnings in worker files - ([be228d0](https://github.com/TraceMachina/nativelink/commit/be228d0d90b41e1d32b2851d594d25a726cadafc))
- Fixes the `entrypoint_cmd` configuration - ([096d7ea](https://github.com/TraceMachina/nativelink/commit/096d7eae802dc4edf4e38251b853917050d470ad))
- Fix a couple of nits with the timestamp additions. - ([b320de5](https://github.com/TraceMachina/nativelink/commit/b320de5ee54595c530ba0078c3f449812cce33d4))

### 📚 Documentation

- Include command example for Powershell in documentation files ([#501](https://github.com/TraceMachina/nativelink/issues/501)) - ([0536d8e](https://github.com/TraceMachina/nativelink/commit/0536d8e4f8f64146941ff789e44043580b98fa16))
- Add CodeQL scanning for Python and JS/TS ([#484](https://github.com/TraceMachina/nativelink/issues/484)) - ([34f0aa0](https://github.com/TraceMachina/nativelink/commit/34f0aa0629bd9ef22fd555bbd9f8c1112af76d9a))
- Add documentation and machine type variables for gcp. ([#457](https://github.com/TraceMachina/nativelink/issues/457)) - ([cb6540c](https://github.com/TraceMachina/nativelink/commit/cb6540c1db55ebe989e53e5159c0284d5e2e82b3))
- Rename docs directory ([#468](https://github.com/TraceMachina/nativelink/issues/468)) - ([43b4ea8](https://github.com/TraceMachina/nativelink/commit/43b4ea82aee98fc570d731019159da4669decb2e))
- Add docs to monorepo ([#453](https://github.com/TraceMachina/nativelink/issues/453)) - ([378b806](https://github.com/TraceMachina/nativelink/commit/378b806f0e877a0566b7a88c7b93799c60a15a64))
- Handle SIGTERM ([#462](https://github.com/TraceMachina/nativelink/issues/462)) - ([e49049c](https://github.com/TraceMachina/nativelink/commit/e49049c9051f5a99a0695930e14497cc74f75165))
- Make Native Link installable via nix ([#442](https://github.com/TraceMachina/nativelink/issues/442)) - ([b8f3ef1](https://github.com/TraceMachina/nativelink/commit/b8f3ef1eab629f7cc973d6f938bc94282001b7ab))
- Adds README to docker-compose deployment-example ([#427](https://github.com/TraceMachina/nativelink/issues/427)) - ([3ec203b](https://github.com/TraceMachina/nativelink/commit/3ec203b9c17e8e4dfa7160f74e948c64e542de16))
- Fix the incorrect config path in the documentation ([#416](https://github.com/TraceMachina/nativelink/issues/416)) - ([7f40696](https://github.com/TraceMachina/nativelink/commit/7f406968e256c5e1b262b992b23400a8cd977241))
- Rewrite the build infrastructure ([#394](https://github.com/TraceMachina/nativelink/issues/394)) - ([3147265](https://github.com/TraceMachina/nativelink/commit/3147265047544572e3483c985e4aab0f9fdded38))
- update the README for discoverability. ([#349](https://github.com/TraceMachina/nativelink/issues/349)) - ([5e2e81a](https://github.com/TraceMachina/nativelink/commit/5e2e81af8999482fef202b50ee880509e8811e6f))
- Minor optimizations and documentation to CacheLookupScheduler - ([66c403d](https://github.com/TraceMachina/nativelink/commit/66c403de197e9af64b91c2f10d82b9709e8919b5))
- Simplify Dockerfile and prepare for Goma example - ([65b8f0e](https://github.com/TraceMachina/nativelink/commit/65b8f0ea37b92c9976dd2cfa445a0835b536a3b8))
- Update README.md - ([7563df7](https://github.com/TraceMachina/nativelink/commit/7563df7a489a926c01bae1d3ec52505db0f49327))
- Document that users should use `-c opt` for release builds - ([9351f26](https://github.com/TraceMachina/nativelink/commit/9351f265f71eca308b18a9ccca2d158f778bba0f))
- Fix bazel version change that broke proto building and documentation - ([1994dde](https://github.com/TraceMachina/nativelink/commit/1994dde8777c718c159823fea93cde89529d1b3c))

### 🧪 Testing & CI

- Fix ensure_full_copy_of_bytes_is_made_test flaky test ([#528](https://github.com/TraceMachina/nativelink/issues/528)) - ([14fdf4f](https://github.com/TraceMachina/nativelink/commit/14fdf4f318240aa735bd0f33fa6d1496513f56ff))
- Add small sleep in some tests to reduce flakes in CI ([#526](https://github.com/TraceMachina/nativelink/issues/526)) - ([fd4e6a3](https://github.com/TraceMachina/nativelink/commit/fd4e6a34a95245ce64abba82ed5f9ae42727ebc5))
- Mark nix-cargo and bazel tests as large ci instances ([#524](https://github.com/TraceMachina/nativelink/issues/524)) - ([a18d2d2](https://github.com/TraceMachina/nativelink/commit/a18d2d2a9e1a1d1ca5f77c305e948d62e7c4a2e1))
- Scale back a few CI runs ([#516](https://github.com/TraceMachina/nativelink/issues/516)) - ([245d9bb](https://github.com/TraceMachina/nativelink/commit/245d9bbdbcdb411077467e14166e01f6e6dfb905))
- Add Kubernetes example ([#479](https://github.com/TraceMachina/nativelink/issues/479)) - ([e1c495f](https://github.com/TraceMachina/nativelink/commit/e1c495fa68b5d85872c98f9231689da4581161b1))
- Avoid writer EOF until fast store complete ([#480](https://github.com/TraceMachina/nativelink/issues/480)) - ([2de8867](https://github.com/TraceMachina/nativelink/commit/2de88676b73116748aac9409d8ca3426d9ab0773))
- Fix pre-commit hooks after 378b806 ([#482](https://github.com/TraceMachina/nativelink/issues/482)) - ([f2bd770](https://github.com/TraceMachina/nativelink/commit/f2bd7704334577da35aa795f81770186873789a6))
- Introduce Local Remote Execution ([#471](https://github.com/TraceMachina/nativelink/issues/471)) - ([449376b](https://github.com/TraceMachina/nativelink/commit/449376b3adb740b65bea661976071629fbd6dcfd))
- Separate CI runs by build system ([#451](https://github.com/TraceMachina/nativelink/issues/451)) - ([75a98f2](https://github.com/TraceMachina/nativelink/commit/75a98f2d8d1e59b4a672925f9853417ada6e06dc))
- Add remaining MacOS targets for further testing ([#450](https://github.com/TraceMachina/nativelink/issues/450)) - ([8f9da8f](https://github.com/TraceMachina/nativelink/commit/8f9da8fea730cb20e3cfb9388256279c64f9ac9c))
- Fix MacOS tests ([#449](https://github.com/TraceMachina/nativelink/issues/449)) - ([befd1b6](https://github.com/TraceMachina/nativelink/commit/befd1b6f8b0776f466bf61f2e6d406814eb757ea))
- Give flaky memory store test more wiggle room ([#448](https://github.com/TraceMachina/nativelink/issues/448)) - ([ab0f1ac](https://github.com/TraceMachina/nativelink/commit/ab0f1ac9dbb9a1a9e4a1894150f79976de84d763))
- Add aarch64-apple-darwin to crates repository supported platforms ([#440](https://github.com/TraceMachina/nativelink/issues/440)) - ([ff6d5cf](https://github.com/TraceMachina/nativelink/commit/ff6d5cfc2a88a3112dc4ffa82aef129fd556437b))
- Fix pre-commit hooks after 3ec203b ([#435](https://github.com/TraceMachina/nativelink/issues/435)) - ([4aa2bc4](https://github.com/TraceMachina/nativelink/commit/4aa2bc4dae5644b448085b9e24fb96a1fb3a58f8))
- Add pre-commit hooks for Starlark ([#414](https://github.com/TraceMachina/nativelink/issues/414)) - ([06654e6](https://github.com/TraceMachina/nativelink/commit/06654e6e01e372e9a87b68f6150f390ca6dfe48b))
- Add default pre-commit hooks ([#405](https://github.com/TraceMachina/nativelink/issues/405)) - ([228bdc4](https://github.com/TraceMachina/nativelink/commit/228bdc45859c77eafdefd1d09840fc7cd21967de))
- Add pre-commit infrastructure ([#401](https://github.com/TraceMachina/nativelink/issues/401)) - ([8de3014](https://github.com/TraceMachina/nativelink/commit/8de30146f382b551ab8dc01d0285e6a206e258b5))
- Added server readiness string listening to integration tests to reduce flakiness ([#378](https://github.com/TraceMachina/nativelink/issues/378)) - ([22abf90](https://github.com/TraceMachina/nativelink/commit/22abf900fa6a6ca53e340d6a5a4ad3279a3bdeb3))
- Refactor sanitizer CI ([#344](https://github.com/TraceMachina/nativelink/issues/344)) - ([ce64cc2](https://github.com/TraceMachina/nativelink/commit/ce64cc286311ef3ceeb84beb3eae33474b4bc4c1))
- Refactor Bazel unit test CI ([#342](https://github.com/TraceMachina/nativelink/issues/342)) - ([ef794c2](https://github.com/TraceMachina/nativelink/commit/ef794c2a14450950837a60ab7090742f73ad898b))
- Integration tests should work for Mac OS. ([#334](https://github.com/TraceMachina/nativelink/issues/334)) - ([1339e9d](https://github.com/TraceMachina/nativelink/commit/1339e9dd439aeba077aaa263873b33e7e157fdd2))
- Make update_protos test compatible with Windows - ([c2e2793](https://github.com/TraceMachina/nativelink/commit/c2e2793bc82a9a68200138c307607d8c805c6207))
- Remove redundant formatting script - ([93572d1](https://github.com/TraceMachina/nativelink/commit/93572d1aac11a93a50758d6f1f8bc9db5b0011c0))
- Attempt to fix the flake Text file busy (os error 26) error in CI - ([b730a90](https://github.com/TraceMachina/nativelink/commit/b730a902de2c842d048c6f437d7f8a1d8a11aa90))
- Attempt to fix flaky tests regarding Text file busy error - ([637c8a9](https://github.com/TraceMachina/nativelink/commit/637c8a97e49c305ccdba303be904a3a8c63a0331))
- Fix flakey tests due to sync_all() not being called - ([6c931fa](https://github.com/TraceMachina/nativelink/commit/6c931fa12749df43eb443f22e81a94c23a205ce8))
- Fix bug in BytestreamServer where it would ignore finish_write - ([f645d69](https://github.com/TraceMachina/nativelink/commit/f645d6906bf4dd07caf36fde37aad27a660390af))
- Files will now close if held open too long - ([67b90e2](https://github.com/TraceMachina/nativelink/commit/67b90e2c9a254687b7525053bb4153f95e216b9d))
- Improve caching in CI and fix flakey prometheus test - ([ea33b6c](https://github.com/TraceMachina/nativelink/commit/ea33b6c7b1e27bf0bcf1f90fc5a4479b6a3854f7))
- Fix incorrect type check. - ([a22e437](https://github.com/TraceMachina/nativelink/commit/a22e437e44d63686fb5819fb370c75f51b9dd513))
- Add TSAN suppression and harness - ([76326db](https://github.com/TraceMachina/nativelink/commit/76326dbf0d8c92b9d233f00ffe3fcef9632049c2))
- Fix ASAN error and enable ASAN in CI - ([e0cc0f9](https://github.com/TraceMachina/nativelink/commit/e0cc0f983341beeda89f80f35392a88d5b2d8e85))
- Add optional sanitizer build configurations - ([a428e23](https://github.com/TraceMachina/nativelink/commit/a428e235090083dca5b6186dcc62aaef4480f4fc))
- Remove need for spawn in BytestreamServer - ([44a4593](https://github.com/TraceMachina/nativelink/commit/44a45932c276c8a871986b65bb9ab33968bf8c6d))
- Enable clippy by default for tests - ([f211ef2](https://github.com/TraceMachina/nativelink/commit/f211ef23a1836f2e0ae25e04832175df87ab23e7))
- Removes needless overoptimization of strings for DigestInfo - ([4062d1d](https://github.com/TraceMachina/nativelink/commit/4062d1db1fad365871d9a3b2efb3cf3a82d5163f))
- Move CI tests to run under docker - ([5322c33](https://github.com/TraceMachina/nativelink/commit/5322c33df1ee48e8c1cb12023f2814e35d0bf780))
- Add convenience config to test clippy - ([1185876](https://github.com/TraceMachina/nativelink/commit/118587684ebc11fbc1bff634a1ad79bb2af2edd4))
- Add a test for filestore loading from disk. - ([5f3e9f5](https://github.com/TraceMachina/nativelink/commit/5f3e9f5d09ac9468cc6d9a57706acc7c79d611b8))
- Remove the callbacks from the filesystem_store - ([e2e62d2](https://github.com/TraceMachina/nativelink/commit/e2e62d20b8badadf20970dde763394310fb24cb7))

### ⚙️ Miscellaneous

- MacOS use non darwin iconv ([#534](https://github.com/TraceMachina/nativelink/issues/534)) - ([2e4a131](https://github.com/TraceMachina/nativelink/commit/2e4a131fb246d16c9d3082b6f231eaad1a85e357))
- MacOS enable flake nix builds ([#529](https://github.com/TraceMachina/nativelink/issues/529)) - ([e1d35d6](https://github.com/TraceMachina/nativelink/commit/e1d35d661801d70c41babf48f9a0a10a8fe975a7))
- Mark GCP & AWS terraform experimental ([#522](https://github.com/TraceMachina/nativelink/issues/522)) - ([910ad03](https://github.com/TraceMachina/nativelink/commit/910ad035ce59d8ba5335c46057fd55ab651fabb0))
- Decouple toolchain generation and execution hashes ([#523](https://github.com/TraceMachina/nativelink/issues/523)) - ([0be8a3a](https://github.com/TraceMachina/nativelink/commit/0be8a3a0950fc6e5810dcb2d83ab348c741134e7))
- Support empty digests in stores ([#507](https://github.com/TraceMachina/nativelink/issues/507)) - ([41a85fb](https://github.com/TraceMachina/nativelink/commit/41a85fb0dfbb0c3f34846ae58fa5110da9fe18c2))
- Increase the Max Blocking Threads ([#510](https://github.com/TraceMachina/nativelink/issues/510)) - ([6d07a86](https://github.com/TraceMachina/nativelink/commit/6d07a86820fe11b5f4a43ee589a6333aea555419))
- Remove beta toolchain from native Cargo builds ([#514](https://github.com/TraceMachina/nativelink/issues/514)) - ([334e738](https://github.com/TraceMachina/nativelink/commit/334e738a078cfa2aadba3d90c93da7f954d3921e))
- Migrate L2 announcements and LB-IPAM to cilium ([#505](https://github.com/TraceMachina/nativelink/issues/505)) - ([df6f5b9](https://github.com/TraceMachina/nativelink/commit/df6f5b94fcc111c66b1f36e7626fd0c7326bcb57))
- Move custom crates to nativelink dirs ([#483](https://github.com/TraceMachina/nativelink/issues/483)) - ([1aadd42](https://github.com/TraceMachina/nativelink/commit/1aadd4291f6e47c28b232b59076b1ee46191c50a))
- Temporarily disable caching in image action ([#466](https://github.com/TraceMachina/nativelink/issues/466)) - ([a22150d](https://github.com/TraceMachina/nativelink/commit/a22150d00864fee7a7d4818a5448319ed936665c))
- Renames the project to nativelink ([#463](https://github.com/TraceMachina/nativelink/issues/463)) - ([2259251](https://github.com/TraceMachina/nativelink/commit/22592519f56daa250bb2d3b5f64eac832d6d2b87))
- Remove caching from native cargo action ([#458](https://github.com/TraceMachina/nativelink/issues/458)) - ([4eab282](https://github.com/TraceMachina/nativelink/commit/4eab2822113b1c405a3d0fae3b736e232f78aab1))
- Migrate logs to tracing ([#387](https://github.com/TraceMachina/nativelink/issues/387)) - ([c5c440a](https://github.com/TraceMachina/nativelink/commit/c5c440a215868581edc563b048a2c297221c22ef))
- Implement API to drain a specific worker ([#413](https://github.com/TraceMachina/nativelink/issues/413)) - ([fbf1e44](https://github.com/TraceMachina/nativelink/commit/fbf1e44874010232f58c9927ad4ab07df34c1f66))
- Use lowercase image names ([#447](https://github.com/TraceMachina/nativelink/issues/447)) - ([8b49bac](https://github.com/TraceMachina/nativelink/commit/8b49bac897e143789615fa7509f76fd389d02d4d))
- Rename ExistanceStore to ExistanceCacheStore ([#437](https://github.com/TraceMachina/nativelink/issues/437)) - ([95f470c](https://github.com/TraceMachina/nativelink/commit/95f470ce16a94c46bf977c45fae6e135861cc899))
- Remove the unnecessary scheduler.rs file ([#431](https://github.com/TraceMachina/nativelink/issues/431)) - ([dd17524](https://github.com/TraceMachina/nativelink/commit/dd1752440abbef2cc3e73c12b1fb0b0d6ef47d19))
- Switch json5 to serde_json5 ([#420](https://github.com/TraceMachina/nativelink/issues/420)) - ([9279ba1](https://github.com/TraceMachina/nativelink/commit/9279ba170824b94f51539311b7bc87bf09688b5a))
- Move action_messages & part of platform_properties to utils crate ([#417](https://github.com/TraceMachina/nativelink/issues/417)) - ([6bc991c](https://github.com/TraceMachina/nativelink/commit/6bc991c30d8dd9abffb38d178ed05cfdf090da79))
- Refactor ExistanceStore ([#418](https://github.com/TraceMachina/nativelink/issues/418)) - ([08b2954](https://github.com/TraceMachina/nativelink/commit/08b29546b3807b91c7ca5a28d7ab84c5bcb6a8e1))
- Remove OpenSSL from build requirements ([#412](https://github.com/TraceMachina/nativelink/issues/412)) - ([55a1292](https://github.com/TraceMachina/nativelink/commit/55a12922bcbb330891a8dc7d82e9eb025881ae5c))
- Remove openssl from flake ([#411](https://github.com/TraceMachina/nativelink/issues/411)) - ([05ec3f2](https://github.com/TraceMachina/nativelink/commit/05ec3f2db673662e63cbc1cfb7860ccff470873d))
- Use Blake3 hasher for computing the action message ([#395](https://github.com/TraceMachina/nativelink/issues/395)) - ([9543964](https://github.com/TraceMachina/nativelink/commit/9543964450abbdb90644335dee6270844a80b99c))
- Rename Turbo Cache to Native Link ([#402](https://github.com/TraceMachina/nativelink/issues/402)) - ([7d9deaf](https://github.com/TraceMachina/nativelink/commit/7d9deaf9ec113b4984dfd7061d1616c1d029d859))
- Reduce allocations needed to hash uploads ([#396](https://github.com/TraceMachina/nativelink/issues/396)) - ([6e0d271](https://github.com/TraceMachina/nativelink/commit/6e0d271583c3adad9b1e7cb67100057dac970452))
- Rename proto organization to trace_machina ([#389](https://github.com/TraceMachina/nativelink/issues/389)) - ([cf5f45b](https://github.com/TraceMachina/nativelink/commit/cf5f45b3a6762cdd607e0a009fbff29615b478e1))
- Migrate from lazy_static to once_cell ([#377](https://github.com/TraceMachina/nativelink/issues/377)) - ([c4b296c](https://github.com/TraceMachina/nativelink/commit/c4b296c4b86cf7a4c5f8a29dd6534a89a5c7685c))
- Move AWS terraform to it's own directory ([#376](https://github.com/TraceMachina/nativelink/issues/376)) - ([dd99eb0](https://github.com/TraceMachina/nativelink/commit/dd99eb0b6113c78243eca1f9916c1282d6cc55b1))
- RBE consistency. ([#364](https://github.com/TraceMachina/nativelink/issues/364)) - ([7966d2d](https://github.com/TraceMachina/nativelink/commit/7966d2d758a7815467831f2c5bfb014c329ac9b7))
- TLS Integration ([#316](https://github.com/TraceMachina/nativelink/issues/316)) - ([6c753d6](https://github.com/TraceMachina/nativelink/commit/6c753d6a5808d6bbaa0ec2f9d6a6e25ff5d92a7a))
- Inline format args in cas_main ([#337](https://github.com/TraceMachina/nativelink/issues/337)) - ([62a2c1e](https://github.com/TraceMachina/nativelink/commit/62a2c1ef34558e3fc4714789d07d22f15099775b))
- Remove futures from scheduler_factory ([#336](https://github.com/TraceMachina/nativelink/issues/336)) - ([f15146d](https://github.com/TraceMachina/nativelink/commit/f15146d0cf94e1a9159f14db775efe4a3e27355d))
- Globally enable windows symlinks ([#327](https://github.com/TraceMachina/nativelink/issues/327)) - ([9d08195](https://github.com/TraceMachina/nativelink/commit/9d0819514cff0bf38c0cdb2f8ddf268cef1305ad))
- propose a PR template to help newcomers get started more easily. - ([b24b214](https://github.com/TraceMachina/nativelink/commit/b24b214ef79fda954125f71d435580059ca04457))
- Always make a copy in MemoryStore to reduce long-lasting allocs - ([699dff9](https://github.com/TraceMachina/nativelink/commit/699dff98437ee0aec31fdaa9918d0b92c612ca16))
- Refactor resource_info to support v2.3 resource names - ([d3d0b64](https://github.com/TraceMachina/nativelink/commit/d3d0b646ae3340fc29e0b44ba75d279f36ab70eb))
- :get_part's writer is now a mutable reference. - ([082a85c](https://github.com/TraceMachina/nativelink/commit/082a85c437fa2d3adf41902b297c68383d3ac81c))
- Future proof code from accidentially creating zombies - ([4bd986c](https://github.com/TraceMachina/nativelink/commit/4bd986c31cdee22d6d65068306ce2229318c741e))
- Remove spawn in dedup store - ([59bbfd8](https://github.com/TraceMachina/nativelink/commit/59bbfd888813364d6492b7f8ff6a7efe1d464a67))
- Change Copyright to Trace Machina, Inc. - ([d8685ed](https://github.com/TraceMachina/nativelink/commit/d8685ed4fc5c53cd98bdfe606d00a8c727144e59))
- Support injecting property values into worker command. - ([06c03de](https://github.com/TraceMachina/nativelink/commit/06c03debe195566f01140b8b59eb4111b7d42093))
- Populate the fast store on partial read - ([e0e0a88](https://github.com/TraceMachina/nativelink/commit/e0e0a883a90489fdb951c76206ba324f7c7f9f8e))
- Worker services now start after listeners to prevent error confiusion - ([16f2ca5](https://github.com/TraceMachina/nativelink/commit/16f2ca53c3b10dac52df565591fe57adc1bf5a0b))
- Output paths are relative to the work directory. - ([f835e6d](https://github.com/TraceMachina/nativelink/commit/f835e6d69952351c5ed819d6c02cd3c472348608))
- improve clarity for getting started. - ([b677d78](https://github.com/TraceMachina/nativelink/commit/b677d788b8f24b2a481abf3b29160725303bc6a1))
- Create CODE_OF_CONDUCT.md - ([a9709b4](https://github.com/TraceMachina/nativelink/commit/a9709b4b84727ac18e1955364c76ebacd73dabba))
- Implement fast slow metrics. - ([10d2618](https://github.com/TraceMachina/nativelink/commit/10d2618b8f43b222fe54907e02c1cba190ba5c5f))
- Make stats in EvictingMap more useful - ([03c1a07](https://github.com/TraceMachina/nativelink/commit/03c1a07752b3ab073df183664b870c396c2c0ed3))
- Removed properties should be known to the property manager. - ([7d0999b](https://github.com/TraceMachina/nativelink/commit/7d0999b8a5454e86735c5bb997db2b7853ad09ed))
- Rename prometheus_utils to metrics_utils - ([298bfb9](https://github.com/TraceMachina/nativelink/commit/298bfb9ace34207adbc2fab07b144ec5d79b89d4))
- Move Cargo.toml to Workspace and config to single library - ([61d89cd](https://github.com/TraceMachina/nativelink/commit/61d89cd04191c5c293af606e5dc9b591cc1fc24e))
- Prometheus now publishes connected clients - ([29fa44a](https://github.com/TraceMachina/nativelink/commit/29fa44ac90d2d04eae6331293a74dc493ae44a8a))
- Streams may now be resumed on disconnect - ([b0b66a1](https://github.com/TraceMachina/nativelink/commit/b0b66a126b6fb85668b098fc3d26678504b1a330))
- Jobs now honor timeout in action - ([fdc6d9b](https://github.com/TraceMachina/nativelink/commit/fdc6d9b55931584deb269b03338460286e78cea7))
- Replace Sha256 with Blake3 in DedupStore - ([b30dfce](https://github.com/TraceMachina/nativelink/commit/b30dfcee1d3eb717a20736c4ec28d20af512208b))
- Improve prometheus utils api - ([87bd0e6](https://github.com/TraceMachina/nativelink/commit/87bd0e63fa21e1a30ac3e788f98ad4b125d3eb2c))
- Change StoreTrait::has to take &[DigestInfo]. - ([732ae83](https://github.com/TraceMachina/nativelink/commit/732ae83422ffd7064f6bdb85cdb25e5d3e131f72))
- Use Axum's router which supports multiple service types - ([4856d35](https://github.com/TraceMachina/nativelink/commit/4856d355a70279d088c0b4dbf01f47dd8c211936))
- Create WaitExecution for ExecutionServer. - ([857bf40](https://github.com/TraceMachina/nativelink/commit/857bf40e18ed2809f0a1bac35128261175f58da1))
- Removes name from action state and make Operation::name schema - ([0fac1ee](https://github.com/TraceMachina/nativelink/commit/0fac1ee5a2ceb255fdec35730ea00fd801851762))
- Implementation of a cache lookup scheduler. - ([fcdf192](https://github.com/TraceMachina/nativelink/commit/fcdf192963e1956e98f8db202b5908fc0ea9f3a4))
- Move to UnorderedFutures in cas_store. - ([5dc31e2](https://github.com/TraceMachina/nativelink/commit/5dc31e255b3e089c416941f88229c345cee6d3ef))
- Implement a GrpcScheduler to forward. - ([bf0b933](https://github.com/TraceMachina/nativelink/commit/bf0b933bd04b806dc5aefb36129b2b7e73cc6aed))
- Promote chosen worker to distribute jobs. - ([2fd1702](https://github.com/TraceMachina/nativelink/commit/2fd1702495a754a6e0ee57fa2198a5ec8544dc42))
- Remove dependency on `satackey/action-docker-layer-caching` - ([0801e26](https://github.com/TraceMachina/nativelink/commit/0801e26e9e506c0016adfaa7f6ddf96dea6b6b84))
- Implement Into traits indirectly via From - ([86d8c60](https://github.com/TraceMachina/nativelink/commit/86d8c60c5a551a3220cde181bf99916c8cacaba7))
- Scheduler is now an interface/trait - ([13159ae](https://github.com/TraceMachina/nativelink/commit/13159ae9657bc589abc7352d9fe4e330367537da))
- Rename `Scheduler` to `SimpleScheduler` - ([d9aeefc](https://github.com/TraceMachina/nativelink/commit/d9aeefca6fed5f294553a4ee2ceb4060ecd4437b))
- Goma places the platform properties in the command, so populate from there. - ([69a9f5f](https://github.com/TraceMachina/nativelink/commit/69a9f5fbe65800c14c063a773527a47f35be1e7a))
- Save action results in the action cache when they are completed on a worker. - ([7bad256](https://github.com/TraceMachina/nativelink/commit/7bad256f8095273a7b0f36aa252ff88ca966474e))
- Move scheduler to use blocking mutexes - ([74b2f04](https://github.com/TraceMachina/nativelink/commit/74b2f049eceebd947ed04180f073ad3bb42e7b4f))
- Change the version of C++ used for protoc to the earliest supported. - ([b62338b](https://github.com/TraceMachina/nativelink/commit/b62338b56d9cad5b6ccaa0299e48331f07f8b2e3))
- Use recent Ubuntu with Clang as build environment - ([5ac2c55](https://github.com/TraceMachina/nativelink/commit/5ac2c5551eeb36f41df3e713a7751fdced5cdc09))
- Rename update_worker_with_internal_error - ([58b5021](https://github.com/TraceMachina/nativelink/commit/58b5021d1bcf4414a1fe6dcb4a313ee19ad56c3d))
- Rename backends.rs to stores.rs - ([451f68f](https://github.com/TraceMachina/nativelink/commit/451f68f36369258f59a88ea768d5f4770d7b5ded))
- Move trivial data access Mutex's to sync Mutex. - ([812b94d](https://github.com/TraceMachina/nativelink/commit/812b94d68475c84f101ced0b26bee74e19eff855))
- Perform parallel reading of content directory to speed up startup. - ([b354b25](https://github.com/TraceMachina/nativelink/commit/b354b2511eafd0f58625904d0bb6c3d45d353080))
- Remove obsolete raze configuration from Cargo.toml - ([42b9ce0](https://github.com/TraceMachina/nativelink/commit/42b9ce0dc18b22203ce9a65731f894faeb690b68))
- Kill currently running actions on scheduler disconnect. - ([d5468dc](https://github.com/TraceMachina/nativelink/commit/d5468dc3965be32ffffa516f4f302269715288bf))
- Workers may be running multiple actions, so track them individually. - ([99ee5b7](https://github.com/TraceMachina/nativelink/commit/99ee5b72ecfb1e343cf9f53c678843fa51fd5365))
- Gzip compression is now disabled by default and configurable - ([f4fcff2](https://github.com/TraceMachina/nativelink/commit/f4fcff29ea6274356d501b3791940ce5c1346ec7))
- Scheduled jobs should be in a queue, not a stack. - ([497446b](https://github.com/TraceMachina/nativelink/commit/497446b04f776ff962b8b75e5e0776944927fca3))
- Migrate from cargo_raze to crate_universe - ([61309c1](https://github.com/TraceMachina/nativelink/commit/61309c1e89ceb5a9880319794d674079cf76fb3e))
- Simplify proto generation - ([eebd6be](https://github.com/TraceMachina/nativelink/commit/eebd6bea6ca80c89cfd185f804320e478b5a3524))
- Overhaul filesystem store to no longer use renameat2 - ([a3cddf9](https://github.com/TraceMachina/nativelink/commit/a3cddf9adb3c287de33cd9b967d8eb99a0c8561a))
- Move from fast-async-mutex to async-lock crate as it's maintained. - ([e172756](https://github.com/TraceMachina/nativelink/commit/e172756613b5398f1ccdaaf258f3f7b80ac4b08e))

### ⬆️ Bumps & Version Updates

- Update dependency mintlify to v4.0.80 ([#536](https://github.com/TraceMachina/nativelink/issues/536)) - ([7564e5e](https://github.com/TraceMachina/nativelink/commit/7564e5e15e39cdf20f5f868a883af8a0ff7b566c))
- Update Rust crate http to ^0.2.11 ([#530](https://github.com/TraceMachina/nativelink/issues/530)) - ([ca146ac](https://github.com/TraceMachina/nativelink/commit/ca146ac97a3a22213af4358e0c2d1ebe8fbee6f9))
- Update native-cargo.yaml Runner Group ([#511](https://github.com/TraceMachina/nativelink/issues/511)) - ([e1843f1](https://github.com/TraceMachina/nativelink/commit/e1843f17c3f957fb8542b6ffcc6784ee2b417ad1))
- Update protobuf dependencies ([#493](https://github.com/TraceMachina/nativelink/issues/493)) - ([3dacdad](https://github.com/TraceMachina/nativelink/commit/3dacdad203c4c2f238e74d6e5beb7401fb312c55))
- Bump trivially bumpable deps ([#488](https://github.com/TraceMachina/nativelink/issues/488)) - ([96302cb](https://github.com/TraceMachina/nativelink/commit/96302cbeab6c59966d3dfd3b99fa0933752d1018))
- Update protos after 1aadd42 ([#489](https://github.com/TraceMachina/nativelink/issues/489)) - ([9c6efe0](https://github.com/TraceMachina/nativelink/commit/9c6efe04acb79e6c75d2d58065d2a8914e3efcc9))
- Make max_bytes_per_stream optional in config ([#474](https://github.com/TraceMachina/nativelink/issues/474)) - ([a01a552](https://github.com/TraceMachina/nativelink/commit/a01a55272f78ef6916e8dfa0532d4b5cb3789036))
- Bump Rust version to 1.74 ([#459](https://github.com/TraceMachina/nativelink/issues/459)) - ([5412d7c](https://github.com/TraceMachina/nativelink/commit/5412d7cc15b48b9871d0e73686c89efc43d35b53))
- Update nightly Rust toolchain for Bazel ([#456](https://github.com/TraceMachina/nativelink/issues/456)) - ([5acfa25](https://github.com/TraceMachina/nativelink/commit/5acfa255703abe2134820881aabeece0efb4edda))
- Update Bazel to 6.4.0 ([#381](https://github.com/TraceMachina/nativelink/issues/381)) - ([2fb59b6](https://github.com/TraceMachina/nativelink/commit/2fb59b61a026416c88a67849435b1d9acd8aa271))
- Update Rust version to 1.73.0 ([#371](https://github.com/TraceMachina/nativelink/issues/371)) - ([56eda36](https://github.com/TraceMachina/nativelink/commit/56eda36661daae5458b2821effcdbcbc9d03b753))
- Reduce flakiness of memory_store_test ([#318](https://github.com/TraceMachina/nativelink/issues/318)) - ([ee1f343](https://github.com/TraceMachina/nativelink/commit/ee1f3436be7db34b0d7adab50e0c29eba9d70968))
- Make memory_store_test compatible with Windows ([#315](https://github.com/TraceMachina/nativelink/issues/315)) - ([2c7e22b](https://github.com/TraceMachina/nativelink/commit/2c7e22b8d5db04ffc9ce2668a7c2cc35da3cc3f6))
- Update rules_rust to 0.29.0 - ([d925e26](https://github.com/TraceMachina/nativelink/commit/d925e264efd7300d0d7c229b015e7ab7019d99dd))
- Update Bazel to 6.3.2 - ([c577db5](https://github.com/TraceMachina/nativelink/commit/c577db5dde9afcb26d24279fe54ae013a1d03730))
- Introduce get_part_ref() and migrate primary use to .get_part() - ([fb6e1fd](https://github.com/TraceMachina/nativelink/commit/fb6e1fd7741852cfe894a9fa7dda1b1106e8cce0))
- Update remote_execution.proto to v2.3 - ([4c71336](https://github.com/TraceMachina/nativelink/commit/4c713362e6876396546c6f02c3dc9d4b181e345e))
- Update all dependencies to their latest versions - ([6a72841](https://github.com/TraceMachina/nativelink/commit/6a7284138c8835ce4abdb61bee3a7d2eb33a7290))
- Update Bazel to 6.2.1 - ([d30571e](https://github.com/TraceMachina/nativelink/commit/d30571ed5135a0901e37dad5ea6283796357d246))
- Update dependencies. - ([85bf34d](https://github.com/TraceMachina/nativelink/commit/85bf34d9adcd4e57b70b1189da56eb1a7a8d1e31))
- Update rules_rust to 0.20.0 - ([7a543c2](https://github.com/TraceMachina/nativelink/commit/7a543c2d832fcd8e17d2227eace4811b22601a43))

## [1.0.1] - 2022-10-17



### ⛰️  Features

- Add support for environmental variable lookup in S3Store config - ([cb0de9e](https://github.com/TraceMachina/nativelink/commit/cb0de9eb40119f7098b4ac0865b4cc5eda8ed374))
- Add ability to use env variables in config files - ([d54b38e](https://github.com/TraceMachina/nativelink/commit/d54b38e213fb243a9b27622894a1529d614a52fb))
- Add Send trait to as_any() store calls - ([c4be423](https://github.com/TraceMachina/nativelink/commit/c4be4239aa8813e238eb76f3efc208fa72f0af0a))
- Add fs module which limits outstanding file handles - ([f7b565f](https://github.com/TraceMachina/nativelink/commit/f7b565f0c525bccd7dc42d529eac64110f15fae5))
- Add functionality for worker to download and create working dir - ([5e7f9ef](https://github.com/TraceMachina/nativelink/commit/5e7f9efece6a8d4ae0288e14f5bda6a04cf594b0))
- Adds .as_any() to stores - ([e5de86d](https://github.com/TraceMachina/nativelink/commit/e5de86d78e7d640d492ef97f7c4b98a1f7e9d358))
- Adds initial implementation for LocalWorker and supporting classes - ([90cff23](https://github.com/TraceMachina/nativelink/commit/90cff230ebb5e7982d780f767aa0b0dc85d87b20))
- Various minor updates - ([cf6dd3d](https://github.com/TraceMachina/nativelink/commit/cf6dd3db5a9633aa9fa3060395266925c09e9a62))
- Add shlex package in third_party - ([d935d7f](https://github.com/TraceMachina/nativelink/commit/d935d7f849a362473aed08347e20607f620589bc))
- Add worker config definitions and rename Metadata to Priority - ([98c4e08](https://github.com/TraceMachina/nativelink/commit/98c4e08e25f1baa0134c61147ee04f736917ef28))
- Add WorkerApiServer to services being served - ([af0ccc3](https://github.com/TraceMachina/nativelink/commit/af0ccc3faa419e37d3e0bde7ff44e3d528617643))
- Add support for keep alive for workers - ([be6f2ee](https://github.com/TraceMachina/nativelink/commit/be6f2ee94b7047d94aef01294b1b37716e80e822))
- [RE] Add WorkerApiService and connection functionality - ([e8a349c](https://github.com/TraceMachina/nativelink/commit/e8a349c991e4bec40fc5435b26d869acbf6a9ac4))
- [RE] Various changes to worker_api.proto - ([86220b7](https://github.com/TraceMachina/nativelink/commit/86220b7429e26ad2b8ba10f877c05baebe3c6d71))
- Add uuid package and update other packages - ([5115bc6](https://github.com/TraceMachina/nativelink/commit/5115bc618be4e1718d437a6be866f57f3bea7099))
- Add SizePartitioningStore - ([d0112be](https://github.com/TraceMachina/nativelink/commit/d0112be4c0deb0ab46bccee8dc074e977336bc74))
- Add RefStore and restructure StoreManager - ([6795bb0](https://github.com/TraceMachina/nativelink/commit/6795bb08d84e53e03f573026b9d97e38a0ac41cc))
- Can now pass json config through CLI & add more sample configs - ([ea4d76d](https://github.com/TraceMachina/nativelink/commit/ea4d76d33fc5130e2b6557f0b8283fe4314adc46))
- Add nix package and upgrade third_party packages - ([a451628](https://github.com/TraceMachina/nativelink/commit/a451628777c34f21d12f95ffdd407a51a8e5a3bb))
- Add basic scaffolding for scheduler + remote execution - ([c91f61e](https://github.com/TraceMachina/nativelink/commit/c91f61edf182f2b64451fd48a5e63fa506a43aae))
- Adds readme to configuration - ([54e8fe7](https://github.com/TraceMachina/nativelink/commit/54e8fe75753876a5feadf800b1b4cfe5dff820d1))
- Add filesystem store - ([d183cad](https://github.com/TraceMachina/nativelink/commit/d183cad24a14b04e2a0c870324f6f5d482db809b))
- Adds simple query_write_status support - ([844014a](https://github.com/TraceMachina/nativelink/commit/844014ac9a8ca246b20a6c3fa861ac970cf94caa))
- Add buf_channel that will be used to help transport bytes around - ([7e111c1](https://github.com/TraceMachina/nativelink/commit/7e111c13bb78ce80b3007aa325839a47790a3341))
- Add byteorder to third_party cargo - ([a76a35f](https://github.com/TraceMachina/nativelink/commit/a76a35f813afa2fe570cb0a59e495c41dcd1004b))
- Adds more eviction templates and functions in prep for filesystem store - ([f2896a7](https://github.com/TraceMachina/nativelink/commit/f2896a798e18569a833fd0d6055bc2d3de59b3a7))
- Adds FastSlow store that will try the fast store before slow store - ([8c71137](https://github.com/TraceMachina/nativelink/commit/8c711376590a6d657b5207d4d318012322f61f30))
- Add dedup store - ([2dba31c](https://github.com/TraceMachina/nativelink/commit/2dba31c44a5baeeefe225b4f5e636b41e4747342))
- Add retry support to get_part in s3_store - ([ea2fc4c](https://github.com/TraceMachina/nativelink/commit/ea2fc4cba95c849e628ecba8b96131aa3378a22e))
- Add CompressionStore and implement LZ4 compression - ([d6cd4f9](https://github.com/TraceMachina/nativelink/commit/d6cd4f91fa1f7d538a10fc11526adfbc05418fb3))
- Add s3 configuration - ([be87381](https://github.com/TraceMachina/nativelink/commit/be87381d05f62e6065c04979f3af7be9a2f222d4))
- Add retry utility in prep for s3_store - ([86e63ee](https://github.com/TraceMachina/nativelink/commit/86e63ee71b0196754774adf23201482a3e272bba))
- Add async_read_taker in prep for s3_store - ([90222f9](https://github.com/TraceMachina/nativelink/commit/90222f958a116aa6df5f366bd0e8ffde266f4f37))
- Add trust_size to DigestInfo - ([d8f218f](https://github.com/TraceMachina/nativelink/commit/d8f218f833fa90410f7feb3c3a9f96f6d2f8eb65))
- Add ability for VerifyStore to check the sha256 hash of the digest - ([40ba2fb](https://github.com/TraceMachina/nativelink/commit/40ba2fb7131dc2946d1adab9f1dfda60b356e282))
- Add sha2 to Cargo.toml in prep for sha256 checking - ([0eb2dab](https://github.com/TraceMachina/nativelink/commit/0eb2dab83722f500c8261b0ab1308c7bf94a77f3))
- Add mock_instant library to Cargo.toml - ([34b9312](https://github.com/TraceMachina/nativelink/commit/34b93120d94d20f0d77b50d9314b98799dd81824))
- Add maplit to third_party dependencies - ([b09153b](https://github.com/TraceMachina/nativelink/commit/b09153b45fa316ebc6c7db2a746430986cd4e8bb))
- Add json package dependencies and updates packages - ([69cf723](https://github.com/TraceMachina/nativelink/commit/69cf72367b78cbe5d6a91c1e9a43902cb0e9fad9))
- Add read stream support - ([5c2db23](https://github.com/TraceMachina/nativelink/commit/5c2db2378ebbd859bdd615ba105c9e3195d8df01))
- Add drop_guard to Cargo.toml - ([3c147cd](https://github.com/TraceMachina/nativelink/commit/3c147cda0de7ed6b2117ac60db0b9d551cd534da))
- Add ability to read partial store - ([0b304cc](https://github.com/TraceMachina/nativelink/commit/0b304cc9fec41fbcffe0b1379f4b4660a6957a1c))
- Add multi-threading and fix some minor performance issues - ([0ed309c](https://github.com/TraceMachina/nativelink/commit/0ed309c0994fe60b6ebfa23024779d3e1170631e))
- Add DigestInfo utility - ([25bef4a](https://github.com/TraceMachina/nativelink/commit/25bef4aa20ac6bf6c8e2af55d5bb7b4055e87e10))
- Add much better way to do error logging with .err_tip() - ([9ae49b6](https://github.com/TraceMachina/nativelink/commit/9ae49b64cabb6ceaf9a4de9718ec123e34d76379))
- Add futures package to Cargo.toml - ([92912e6](https://github.com/TraceMachina/nativelink/commit/92912e6cc786a9716fd29469dab81c603e7718f9))
- Add Capabilities and Execution api endpoints - ([24dec02](https://github.com/TraceMachina/nativelink/commit/24dec02fe054da8ba3862f8e5057e6a0f42998ed))
- Add ./rust_fmt.sh - ([5c65005](https://github.com/TraceMachina/nativelink/commit/5c650052e6edf35246c00513e58d7c0fe19e91fc))
- Add dependent proto files for bazel cas - ([d845d40](https://github.com/TraceMachina/nativelink/commit/d845d404fdc07bd848ea057f7fa7260dc877fb13))

### 🐛 Bug Fixes

- Fix bug if no instance_name/resource_name is given upload does not work - ([b010b4b](https://github.com/TraceMachina/nativelink/commit/b010b4bd019e3e4cce5e5115b0ff797c45e85d96))
- Fix scheduler so platform properties are properly restored - ([059b0ef](https://github.com/TraceMachina/nativelink/commit/059b0ef90474ffbb7839fa3764db9dcb31b21cf5))
- Fix bug on output_files' folders were not being created - ([bb010f2](https://github.com/TraceMachina/nativelink/commit/bb010f2fffca465a6af9afd21db61ae9b2212534))
- Fix bug where worker was not creating working directory properly - ([4e51b6d](https://github.com/TraceMachina/nativelink/commit/4e51b6d80e284de5d0f7dfcf469900e1af2b610b))
- Fix wrong `type_url` in google-proto's Any type - ([9cda96a](https://github.com/TraceMachina/nativelink/commit/9cda96a654fed9d997b9ac179f7a69b28af8b6de))
- Fix bug during .has() call in dedup store - ([5cc9a09](https://github.com/TraceMachina/nativelink/commit/5cc9a09dcf2330d993c68a7510871e17d4321227))
- Fixed various bugs in filesystem store - ([7ba407d](https://github.com/TraceMachina/nativelink/commit/7ba407d24533a397b49c39f7ee5eb42f3a951415))
- Fix bug in evicting_map with unref improperly called and readability - ([ea393a5](https://github.com/TraceMachina/nativelink/commit/ea393a520f57c8d23aba565317d56ecce7aa80b8))
- Fix minor issue in FastSlowStore - ([81fb378](https://github.com/TraceMachina/nativelink/commit/81fb378e0c3d894694c7a830f05b37035393edb2))
- Fix case where s3 uploads in wrong order - ([4798fe9](https://github.com/TraceMachina/nativelink/commit/4798fe9d7130e98ebeda5a8c27512b042a1058c0))
- Fix bug in s3_store where 5mb is calculated wrong & improve debugability - ([0451781](https://github.com/TraceMachina/nativelink/commit/0451781a8ab55ddaa93d577e8ceb49daaa1bca62))
- Fix s3_store - ([efcb653](https://github.com/TraceMachina/nativelink/commit/efcb653ae741f97eb1e65272decc6842e33b424b))
- Fixed AsyncFixedBuffer - ([519fa9f](https://github.com/TraceMachina/nativelink/commit/519fa9f2c49edb2054a9263940bfa350b4c62306))
- Minor changes to AsyncFixedBuffer - ([a506363](https://github.com/TraceMachina/nativelink/commit/a506363c8a4b8c8171982b4edcb1fbc6eef1f8ac))
- Fix lifetime of StoreTrait::update() - ([9ec43a2](https://github.com/TraceMachina/nativelink/commit/9ec43a2d5bf408b419fb7a75d976f6668888dc6f))
- Fix --config debug config to properly add debug symbols - ([90b43c6](https://github.com/TraceMachina/nativelink/commit/90b43c6a5e056543b341004e28385b88b2fca39a))
- Fix small bug in gen_rs_proto - ([627c0f8](https://github.com/TraceMachina/nativelink/commit/627c0f8ed7bf1098f99fd756c440005a98b2579a))
- Fix small needless cast to i64 - ([59c609e](https://github.com/TraceMachina/nativelink/commit/59c609e71977a0d3822f85730d4b7844780a366d))
- Fix bug with verify_store when receiving multiple chunks - ([a78caec](https://github.com/TraceMachina/nativelink/commit/a78caec3927fe6c1b4fdd8bf207013125ff72a30))
- Fixed typo in debug message when instance_name is not properly set - ([d231ea1](https://github.com/TraceMachina/nativelink/commit/d231ea1f08802e09a1b1f3501b8368d844643a45))
- Fixed EOF bits and few other items in order to get bazel working - ([8558ee9](https://github.com/TraceMachina/nativelink/commit/8558ee9b51644782eb726638226e338b7605f465))
- Fix async_fixed_buffers to add get_closer() - ([9225b1f](https://github.com/TraceMachina/nativelink/commit/9225b1fb0c75ed9fd54fa584682eb1bbba3dbab0))
- Fix memory leak - ([c27685c](https://github.com/TraceMachina/nativelink/commit/c27685c2f7846cb2868bc5ecae9fd697c9e7c1bb))
- Fix Store import in cas_server.rs - ([a7e7859](https://github.com/TraceMachina/nativelink/commit/a7e7859d485712a7857b7d5a55178e03a8a403a9))

### 📚 Documentation

- Add terraform deployment example and documentation - ([c7dff9f](https://github.com/TraceMachina/nativelink/commit/c7dff9f48169171696fa42654823e6beb82dd6c3))
- Filesystem store now delays before deleting temp file - ([33d88c5](https://github.com/TraceMachina/nativelink/commit/33d88c5d24943bc7bc134dfbbb6cbd91c62b400a))
- Support deprecated symlink fields & fix bug for workers use CWD - ([00431f9](https://github.com/TraceMachina/nativelink/commit/00431f947b358a7dc95400a361307521c9d1c5ad))
- FastSlowStore now properly documented and used in LocalWorkerConfig - ([728cb90](https://github.com/TraceMachina/nativelink/commit/728cb90c7765f94460197113feb6d9c7ae6c514b))

### 🧪 Testing & CI

- Adds GrpcStore and first integration tests - ([117e173](https://github.com/TraceMachina/nativelink/commit/117e1733b81e8f71d28dec324a7d9dffd79cb1ca))
- Fix bug in scheduler of not removing actions after execution - ([f2b825b](https://github.com/TraceMachina/nativelink/commit/f2b825bf436bddb7d24c076b1efc165e5809ff61))
- Fixes flakey filesystem_store_test - ([717d87a](https://github.com/TraceMachina/nativelink/commit/717d87a89b0ee855c45b6ee6a07c1eafe43029a7))
- First draft to get remote execution working - ([f207dfa](https://github.com/TraceMachina/nativelink/commit/f207dfaf41226ec568720534c1d28ca2d57ef634))
- Restructure LocalWorker for easier testing - ([d7d71a1](https://github.com/TraceMachina/nativelink/commit/d7d71a138269ee71d31e9816d6ae2dd90ecd65bc))
- Fix bug in memory store when receiving a zero byte object - ([52445a1](https://github.com/TraceMachina/nativelink/commit/52445a1c234cef5f065d76c0af938b5744dc732d))
- Fix github CI badge - ([2758d22](https://github.com/TraceMachina/nativelink/commit/2758d22a086da3a9d16546b702598597cdea2bf9))
- Adds automated CI tests on pull requests and master - ([e647de0](https://github.com/TraceMachina/nativelink/commit/e647de0ba650bac1b2c785327e34ccb53d68a5d5))
- Add more basic scheduler support - ([2edf514](https://github.com/TraceMachina/nativelink/commit/2edf514742e27cba2bc12c74539463494800a29c))
- Dedup store will now bypass deduplication when size is small - ([997be53](https://github.com/TraceMachina/nativelink/commit/997be53c7560bb0dca8fe2ab08831ec172ede7a6))
- Fix buf in bytestream_server when NotFound was returned - ([a4634eb](https://github.com/TraceMachina/nativelink/commit/a4634ebf54f2ee4ad8b154c2ed2e5f4e29f8d23a))
- Upgrade rustc, use new nightly, rules_python, and rustfmt - ([d0c31fb](https://github.com/TraceMachina/nativelink/commit/d0c31fb3b224921a58a9da5e9d746ceb192e9b71))
- Fix format of util/tests/async_read_taker_test.rs - ([cd12d1d](https://github.com/TraceMachina/nativelink/commit/cd12d1da698d932775ffc32802855a2c3297675b))
- dummy_test.sh will now print some equal signs when done - ([1227d39](https://github.com/TraceMachina/nativelink/commit/1227d39d4b995e1127743be333e4890220d8aa21))
- Added single_item_wrong_digest_size test back to stable - ([b517db1](https://github.com/TraceMachina/nativelink/commit/b517db148d1c807bfdc84916801ae3926e805384))
- Add //:dummy_test that is useful for testing caching - ([e5a1e9a](https://github.com/TraceMachina/nativelink/commit/e5a1e9ad82b2b910738798764e0f367d76496122))
- Add dummy test that is used for easy caching - ([efd449a](https://github.com/TraceMachina/nativelink/commit/efd449afd665f16f21c81f5618e294658e8e7d32))
- Add test for bytestream::write() - ([5dc8ac0](https://github.com/TraceMachina/nativelink/commit/5dc8ac0d64a7241bc4f1c54d1376a9f870dfca8c))
- Add bytestream server scaffolding - ([7aff76f](https://github.com/TraceMachina/nativelink/commit/7aff76f755b731a99adae5f4c2a512c0cf8c5476))
- Add test for single item update action cache - ([c3d89e1](https://github.com/TraceMachina/nativelink/commit/c3d89e1981d4184928086d5643594b77d3fad433))
- get_action_result done with tests - ([fcc8a31](https://github.com/TraceMachina/nativelink/commit/fcc8a319f9f4c061612ee43de58e46cea730a2d9))
- Add first test for ac_server - ([221ed5f](https://github.com/TraceMachina/nativelink/commit/221ed5fbd765c92f7277a1da074563836689c867))
- Add test and fix bug when querying and using bad hash on .has() - ([9adbe81](https://github.com/TraceMachina/nativelink/commit/9adbe81aa401bb067f3fca0aeb35a3433b2cf97b))
- Add test for batch_read_blobs - ([4b1ae1a](https://github.com/TraceMachina/nativelink/commit/4b1ae1ae70118b8b3b324201c46466b106fe206e))
- Add tests for invalid memory store requests - ([4f8e5a7](https://github.com/TraceMachina/nativelink/commit/4f8e5a7e2cacd8bcc4370ba3c55825398292c826))
- Add impl and tests for get store data - ([7922f84](https://github.com/TraceMachina/nativelink/commit/7922f8439c2cb59b7f888f409876971a6c0d59aa))
- Basic HashMap for memory store and enable store_one_item_existence test - ([5206e74](https://github.com/TraceMachina/nativelink/commit/5206e742b3294633864252e3ff6341d84dd08d64))
- Add test for store_one_item_existence - ([a6f1a70](https://github.com/TraceMachina/nativelink/commit/a6f1a70cb81de2ef0fe74cdb08401a1cd6828ffe))
- Add store and first test - ([ed4bde4](https://github.com/TraceMachina/nativelink/commit/ed4bde4310ddedff0e5473295410f1f3d68fce71))
- Add ability to resolve GetCapabilities and bazel connect testing - ([1aba20c](https://github.com/TraceMachina/nativelink/commit/1aba20c23f2db10277e50cb1ee8ecb51c04c2e10))

### ⚙️ Miscellaneous

- Change license to Apache 2 license - ([1147525](https://github.com/TraceMachina/nativelink/commit/11475254245224de09647d130ad078f0abc35168))
- Remove dependency on rust-nightly - ([41028a9](https://github.com/TraceMachina/nativelink/commit/41028a956dd5eeac7166a25b56a7b96a401a2045))
- Enable Gzip compression support to GRPC - ([438afbf](https://github.com/TraceMachina/nativelink/commit/438afbfc2337dc10d6003d169a6c5419e3acce56))
- Make JSON environmental variable lookup much better - ([16a1a18](https://github.com/TraceMachina/nativelink/commit/16a1a1838eceda4d2765a132e9d3cac19d78f2e5))
- Support more platforms for compilation - ([7e09945](https://github.com/TraceMachina/nativelink/commit/7e09945f0eacdf3b5c0d95a95dda2a7155eee644))
- Scheduler will retry on internal errors - ([2be02e2](https://github.com/TraceMachina/nativelink/commit/2be02e2fd7b99d205bce9817c57f816f51ce3720))
- LocalWorker will now purge the work directory on construction - ([7325ffb](https://github.com/TraceMachina/nativelink/commit/7325ffbcabf064efb4f964cd1994fed5768a6f67))
- Upgrade dependencies - ([0765b63](https://github.com/TraceMachina/nativelink/commit/0765b63b45b711f1c61c382482c5cff8f25f94a5))
- Upgrade LRU package and other deps - ([1c02c09](https://github.com/TraceMachina/nativelink/commit/1c02c09d1cca38327c4232f7ca0d254575cf8022))
- `is_executable` flag on files now overrides unix_mode exec flag - ([420e7ee](https://github.com/TraceMachina/nativelink/commit/420e7ee32ae1d8f3075e9104e00701ad76baf1cf))
- Remove the requirement of using reference for action_messages - ([5daa4ff](https://github.com/TraceMachina/nativelink/commit/5daa4ff29373593daad58292d14fbbe2478fc5d7))
- Services in main() now run in their own spawn - ([09c52d9](https://github.com/TraceMachina/nativelink/commit/09c52d9de26c8a903575e407625e71caaeb8f92f))
- Implement execution_response() in WorkerApiServer - ([e26549a](https://github.com/TraceMachina/nativelink/commit/e26549aa5aab19cd54e0d6e7270c3f4e2e20954d))
- Implement going_away() support to WorkerAPIServer - ([0b68053](https://github.com/TraceMachina/nativelink/commit/0b6805332c80735068bf86f3fbc6fa31e5bf4c69))
- Rename project turbo-cache - ([76a7131](https://github.com/TraceMachina/nativelink/commit/76a713127573841fa2b9c9c27e0adefae373bad6))
- Remove small item optimization in dedup_store and .has() calls - ([f9e090d](https://github.com/TraceMachina/nativelink/commit/f9e090db200e87a434c64e9aebead363a224adad))
- S3 now has a limit on number of active requests at a time - ([025305c](https://github.com/TraceMachina/nativelink/commit/025305c13b23bf8d4661359cc7fae49450ac5947))
- Cleanup un-needed lifetime annotations in store traits - ([e341583](https://github.com/TraceMachina/nativelink/commit/e341583703684dd32dcc38ba76bba045ffee5c65))
- Refactor code to use streams instead of AsyncRead/AsyncWrite - ([697a11c](https://github.com/TraceMachina/nativelink/commit/697a11cb1bcbdfb9ddfd0deecb52c6066388f428))
- Move evicting_map to use bytes instead of Vec for future efficencies - ([9abc64c](https://github.com/TraceMachina/nativelink/commit/9abc64cfa50290778779e0b13348d2b15b410143))
- Minor change to make fastcdc return Bytes intead of BytesMut - ([d5f1918](https://github.com/TraceMachina/nativelink/commit/d5f191812eb09598f3335f15bff5b18580020b25))
- EvictingMap is now a template - ([791dc67](https://github.com/TraceMachina/nativelink/commit/791dc67d1c2800beb5758dab4ff9c12e7644ddb7))
- `.has()` now returns size on success instead of boolean - ([6a88752](https://github.com/TraceMachina/nativelink/commit/6a887525436a93dd68d3aea4219f092ed8c2101c))
- Slightly improve the performance of fastcdc by ~30% - ([3063d2c](https://github.com/TraceMachina/nativelink/commit/3063d2c53c12a9eb5c1a86c1c17d499a09f8d3cf))
- Stores now support non-exact upload sizes & S3 store optimizations - ([d093fdd](https://github.com/TraceMachina/nativelink/commit/d093fddeb927b2ba453a061ca4da4aefe3572e8c))
- Upgrade deps packages (tonic, tokio, tonic) - ([692070a](https://github.com/TraceMachina/nativelink/commit/692070a1bc596213bd18c370b72459cdce4b6888))
- Various changes to improve debugging - ([3a4d743](https://github.com/TraceMachina/nativelink/commit/3a4d743afb2bad3de416288e4b0e13cdc94d4bf1))
- Remove DigestInfo.trust_size & add expected_size to update() function - ([e8a83eb](https://github.com/TraceMachina/nativelink/commit/e8a83ebe5a240750b5387aa069f3a0697c102ea1))
- Use fast-async-mutex for all mutex needs - ([c5d450c](https://github.com/TraceMachina/nativelink/commit/c5d450cefdc4beeb32a4645c024090e7c7b6fa85))
- Minor changes to ac_server to reduce chance of allocations - ([41f989f](https://github.com/TraceMachina/nativelink/commit/41f989fb1108e02374277805d1c65dc63a14cfc0))
- Upgrade cargo packages and rust version - ([7956a02](https://github.com/TraceMachina/nativelink/commit/7956a023be9646597a97b9ca2dcafc798d391deb))
- Remove Sync requirement for ResultFuture - ([59720c9](https://github.com/TraceMachina/nativelink/commit/59720c98c8ae664333f31b3b097a8db7c6f435e9))
- Upgrade tonic, tokio and prost - ([14eebc6](https://github.com/TraceMachina/nativelink/commit/14eebc612eeb6df184017e48796d6226800cc0ff))
- MemoryStore now properly honors HASH and size constraints - ([52b9ddb](https://github.com/TraceMachina/nativelink/commit/52b9ddbbdd81e20eacf436f3660054e5d5dae57a))
- Move EvictingMapt to use DigestInfo - ([1148952](https://github.com/TraceMachina/nativelink/commit/11489528e1c071b359c39dfacafb0847bbe148f3))
- MemoryStore now can be configured to evict entries - ([5830d0b](https://github.com/TraceMachina/nativelink/commit/5830d0bb32e0066292253ea5668c5b4c36d17104))
- ByteStream service now properly honors instance_name - ([e9c8915](https://github.com/TraceMachina/nativelink/commit/e9c8915abc48705719f18ebf3dfc64ec9e2995c7))
- Action Cache service now properly honors instance_name - ([7af7cc6](https://github.com/TraceMachina/nativelink/commit/7af7cc6500254f38e705ed00c28ea2927549419a))
- Remove need to pass CAS store to action cache service - ([cc37b08](https://github.com/TraceMachina/nativelink/commit/cc37b08d49a94b7cb67a0f6f60aed5ee32711c62))
- CAS service now supports instance name partitioning - ([faba6c9](https://github.com/TraceMachina/nativelink/commit/faba6c99c384e848b28d30cd24629ffbeb45df78))
- Moved hard coded settings to config - ([0f7b2d0](https://github.com/TraceMachina/nativelink/commit/0f7b2d0c74fc238e7254d3788dae1ff8aaf5b378))
- Configuration is now loaded from json - ([99d170c](https://github.com/TraceMachina/nativelink/commit/99d170c9e5190a84d06d4e182f21747fa438c6c3))
- Move store to use Pin and DigestInfo is no longer ref - ([cf8cb79](https://github.com/TraceMachina/nativelink/commit/cf8cb79db8277d209c1fce1d05070ead2892ffbb))
- Now rust_fmt.sh will compile dependencies if needed - ([c93b852](https://github.com/TraceMachina/nativelink/commit/c93b8527a131198774b1afb8d8d4fe39e8dd81ed))
- Now use opt by default and add --config debug flag for dbg in bazel - ([2abea44](https://github.com/TraceMachina/nativelink/commit/2abea44f49fcbf86797849e778c5b908fd79c2e0))
- Better logging - ([e61482a](https://github.com/TraceMachina/nativelink/commit/e61482a1aeebdc69011a669fefb2b6f8ad66691d))
- Improve logging - ([7068b4b](https://github.com/TraceMachina/nativelink/commit/7068b4b2757ffafbcfd1c02fe8374cf16c54e7db))
- Store now has flag of when to verify the size of the digest - ([6c70370](https://github.com/TraceMachina/nativelink/commit/6c70370e917bf0e8093bbb80cb859b0cde6fd8cb))
- Implement bufferstream's write() - ([e09db45](https://github.com/TraceMachina/nativelink/commit/e09db451954a86695c05c6214482c694e1344b07))
- Move AC to inner functions - ([5452f4b](https://github.com/TraceMachina/nativelink/commit/5452f4b617c4573d155abfbc4cd9aedceac75353))
- Upgrade tokio library - ([3e8c210](https://github.com/TraceMachina/nativelink/commit/3e8c210adc3696fa552bf14d4756ee57ee71d37a))
- Implement get_action_result for action cache - ([04d6a6d](https://github.com/TraceMachina/nativelink/commit/04d6a6dd8f262d88c3c71da771c07f699bf2bfd8))
- Remove need for exposing store in CasServer - ([b4682cb](https://github.com/TraceMachina/nativelink/commit/b4682cb0ed1366ba91b272da9055ec736e919047))
- Makes storage an Arc in prep for action cache good-ness - ([365151a](https://github.com/TraceMachina/nativelink/commit/365151ab8cbc5af62e6577b7910a1ec704cdab75))
- Cas batch_read_blobs now works - ([42d4c8d](https://github.com/TraceMachina/nativelink/commit/42d4c8d69f62c4260199edf44d160d2b0a0b738e))
- Move some code to common util library - ([2048d24](https://github.com/TraceMachina/nativelink/commit/2048d24192928a6fa570cef7ed3db485ea8c4b88))
- Move macros to //util folder - ([2fe5d30](https://github.com/TraceMachina/nativelink/commit/2fe5d30bdad83b8644a987b05679df26417dcc71))
- Cleanup capabilities a little - ([4f5aad9](https://github.com/TraceMachina/nativelink/commit/4f5aad97de4e772b0892998daa2c0c060e4d35a0))
- CAS server moved to cas directory - ([fcca6df](https://github.com/TraceMachina/nativelink/commit/fcca6dfe6e358b41543c1dea1d5fe6922cde0828))
- Ignore target directory and ./rust_fmt.sh uses parallel now - ([7a070b9](https://github.com/TraceMachina/nativelink/commit/7a070b9a681614f06869584c0e8dd986078276ca))
- Bazel remote execution server mock - ([862a923](https://github.com/TraceMachina/nativelink/commit/862a9236d22244a38f22c492b7cf71fd307207da))
- Initial commit - ([4bf7887](https://github.com/TraceMachina/nativelink/commit/4bf788777db36f055a83c1b59e41102418e529c3))

### ⬆️ Bumps & Version Updates

- Add minimum bazel version to .bazelversion - ([a2be6f5](https://github.com/TraceMachina/nativelink/commit/a2be6f5a902c28c270fc8a09cb2c26a85587044a))
- Updates cargo packages - ([a610e69](https://github.com/TraceMachina/nativelink/commit/a610e69ea37e3cc281df3ee5f066e9f901ffa3a5))
- Various minor changes - ([2546a77](https://github.com/TraceMachina/nativelink/commit/2546a7797cce995173c37b084d849b2c7080bdbc))
- Upgrade prost & tonic - ([baad561](https://github.com/TraceMachina/nativelink/commit/baad56167691f168346258cdac58c1f2afbce18c))
- Adds ability in proto for worker to return internal errors - ([576aff4](https://github.com/TraceMachina/nativelink/commit/576aff4d0a7a654094cf07c685f4d67b30875808))
- Restructure config format in prep for WorkerApiServer - ([e8ce3a8](https://github.com/TraceMachina/nativelink/commit/e8ce3a809df2c33d5a10f6971fb823743a21d37b))
- Update Readme - ([ecb4dc5](https://github.com/TraceMachina/nativelink/commit/ecb4dc58a655ae751244621b2445a4d3c6d51495))
- Update third_party/Cargo in prep for s3_store - ([f00b58f](https://github.com/TraceMachina/nativelink/commit/f00b58fb217419b8777348a78ec96a5a297ace8d))
- Update cargo packages - ([786552f](https://github.com/TraceMachina/nativelink/commit/786552fb6f300d4e532c1838b5675720c57e3ebd))
- Move to 120 character line limit - ([feb392d](https://github.com/TraceMachina/nativelink/commit/feb392d75a52054e2af77bafdd59eb023b529b6e))
- Update fully works for CAS w/ tests - ([d2ed91e](https://github.com/TraceMachina/nativelink/commit/d2ed91e1498e7a89d7cefe2d90123f19fd55ead9))

<!-- generated by git-cliff -->
<!-- vale on -->
