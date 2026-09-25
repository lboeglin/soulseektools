//! `stk doctor`: check the configuration, the library roots and slskd.

use serde_json::json;

use crate::cli::context::{show, table, AppContext};
use crate::errors::Result;

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
    required: bool,
}

impl Check {
    fn status(&self) -> &'static str {
        if self.ok {
            "ok"
        } else if self.required {
            "missing"
        } else {
            "optional"
        }
    }
}

fn checks(context: &AppContext) -> Vec<Check> {
    let settings = context.settings();
    let roots = context.library_roots();
    let index_path = &settings.index.path;

    vec![
        Check {
            name: "config",
            ok: context.resolved.source_path.is_some(),
            detail: context
                .resolved
                .source_path
                .as_ref()
                .map_or_else(|| "using defaults".to_owned(), |path| show(path)),
            required: false,
        },
        Check {
            name: "library root",
            ok: roots.iter().all(|root| root.is_dir()),
            detail: roots
                .iter()
                .map(|root| show(root))
                .collect::<Vec<_>>()
                .join(", "),
            required: true,
        },
        Check {
            name: "library index",
            ok: index_path.parent().is_some_and(std::path::Path::is_dir) || !index_path.exists(),
            detail: show(index_path),
            required: true,
        },
        Check {
            name: "slskd url",
            ok: !settings.slskd.url.is_empty(),
            detail: settings.slskd.url.clone(),
            required: true,
        },
        Check {
            name: "slskd api key",
            ok: settings.slskd.api_key.is_some(),
            detail: if settings.slskd.api_key.is_some() {
                "set".to_owned()
            } else {
                "export SLSKD_API_KEY to enable downloads".to_owned()
            },
            required: false,
        },
        Check {
            name: "discogs token",
            ok: settings.wantlist.discogs_token.is_some(),
            detail: if settings.wantlist.discogs_token.is_some() {
                "set".to_owned()
            } else {
                "export DISCOGS_TOKEN to read a collection at 60 releases a minute rather than 25"
                    .to_owned()
            },
            required: false,
        },
        Check {
            name: "lastfm api key",
            ok: settings.wantlist.lastfm_api_key.is_some(),
            detail: if settings.wantlist.lastfm_api_key.is_some() {
                "set".to_owned()
            } else {
                "export LASTFM_API_KEY to read lastfm:<user>".to_owned()
            },
            required: false,
        },
    ]
}

pub fn run(context: &AppContext) -> Result<i32> {
    let checks = checks(context);

    if context.as_json {
        context.emit(&json!(checks
            .iter()
            .map(|check| json!({
                "name": check.name,
                "status": check.status(),
                "detail": check.detail,
                "ok": check.ok,
            }))
            .collect::<Vec<_>>()));
    } else {
        let rows: Vec<Vec<String>> = checks
            .iter()
            .map(|check| {
                let marker = if check.ok { "OK" } else { "--" };
                vec![
                    marker.to_owned(),
                    check.name.to_owned(),
                    check.detail.clone(),
                ]
            })
            .collect();
        context.say(&table(&["", "Check", "Detail"], &rows));
    }

    let broken = checks.iter().any(|check| !check.ok && check.required);
    Ok(i32::from(broken))
}
