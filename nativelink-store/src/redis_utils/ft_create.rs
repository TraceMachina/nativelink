// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
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

use redis::RedisError;
use redis::aio::ConnectionLike;

pub(crate) struct SearchSchema {
    pub field_name: String,
    pub sortable: bool,
}

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct FtCreateOptions {
    pub prefixes: Vec<String>,
    pub nohl: bool,
    pub nofields: bool,
    pub nofreqs: bool,
    pub nooffsets: bool,
    pub temporary: Option<u64>,
}

pub(crate) async fn ft_create<C>(
    mut connection_manager: C,
    index: String,
    options: FtCreateOptions,
    schemas: Vec<SearchSchema>,
) -> Result<(), RedisError>
where
    C: ConnectionLike + Send,
{
    let mut cmd = redis::cmd("FT.CREATE");
    let mut ft_create_cmd = cmd.arg(index).arg("ON").arg("HASH");
    if options.nohl {
        ft_create_cmd = ft_create_cmd.arg("NOHL");
    }
    if options.nofields {
        ft_create_cmd = ft_create_cmd.arg("NOFIELDS");
    }
    if options.nofreqs {
        ft_create_cmd = ft_create_cmd.arg("NOFREQS");
    }
    if options.nooffsets {
        ft_create_cmd = ft_create_cmd.arg("NOOFFSETS");
    }
    if let Some(seconds) = options.temporary {
        ft_create_cmd = ft_create_cmd.arg("TEMPORARY").arg(seconds);
    }
    if !options.prefixes.is_empty() {
        ft_create_cmd = ft_create_cmd.arg("PREFIX").arg(options.prefixes.len());
        for prefix in options.prefixes {
            ft_create_cmd = ft_create_cmd.arg(prefix);
        }
    }
    ft_create_cmd = ft_create_cmd.arg("SCHEMA");
    for schema in schemas {
        ft_create_cmd = ft_create_cmd.arg(schema.field_name).arg("TAG");
        if schema.sortable {
            ft_create_cmd = ft_create_cmd.arg("SORTABLE");
        }
    }

    ft_create_cmd
        .to_owned()
        .exec_async(&mut connection_manager)
        .await?;
    Ok(())
}
