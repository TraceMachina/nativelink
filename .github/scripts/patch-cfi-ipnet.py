#!/usr/bin/env python3
"""Avoid an ipnet 2.11.0 parser-dyn-dispatch CFI failure in nightly CI.

Patch only the unpacked crates.io source on the disposable CI runner.
The parser tries IPv4 and IPv6 in the same order, preserving atomic rollback;
no checks are disabled and the repository's production dependency is unchanged.
"""

import os
from pathlib import Path


OLD_READ_OR = """    // Return result of first successful parser
    fn read_or<T>(&mut self, parsers: &mut [Box<dyn FnMut(&mut Parser) -> Option<T> + 'static>])
               -> Option<T> {
        for pf in parsers {
            if let Some(r) = self.read_atomically(|p: &mut Parser| pf(p)) {
                return Some(r);
            }
        }
        None
    }

"""

OLD_READ_IP_NET = """    fn read_ip_net(&mut self) -> Option<IpNet> {
        let ipv4_net = |p: &mut Parser| p.read_ipv4_net().map(IpNet::V4);
        let ipv6_net = |p: &mut Parser| p.read_ipv6_net().map(IpNet::V6);
        self.read_or(&mut [Box::new(ipv4_net), Box::new(ipv6_net)])
    }
"""

NEW_READ_IP_NET = """    fn read_ip_net(&mut self) -> Option<IpNet> {
        // Keep the same fallback/rollback semantics, but avoid boxed FnMut
        // dispatch that rustc nightly misidentifies under CFI instrumentation.
        self.read_atomically(|p| p.read_ipv4_net().map(IpNet::V4))
            .or_else(|| self.read_atomically(|p| p.read_ipv6_net().map(IpNet::V6)))
    }
"""


def patch(parser_source: str) -> str:
    for anchor in (OLD_READ_OR, OLD_READ_IP_NET):
        if parser_source.count(anchor) != 1:
            raise RuntimeError("ipnet 2.11.0 parser changed; refusing unreviewed CFI workaround")
    return parser_source.replace(OLD_READ_OR, "", 1).replace(
        OLD_READ_IP_NET, NEW_READ_IP_NET, 1
    )


def main() -> None:
    cargo_home = Path(os.environ.get("CARGO_HOME", "~/.cargo")).expanduser()
    matches = list((cargo_home / "registry" / "src").glob("*/ipnet-2.11.0/src/parser.rs"))
    if len(matches) != 1:
        raise RuntimeError(f"expected one unpacked ipnet 2.11.0 parser, found {len(matches)}")
    parser_path = matches[0]
    parser_path.write_text(patch(parser_path.read_text()))
    print(f"CFI workaround: use direct IPv4/IPv6 parser calls in {parser_path}")


if __name__ == "__main__":
    main()
