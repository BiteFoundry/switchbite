use serde_json::json;
use switchbite::catalog::{shortlist, snapshot};

fn c(id: &str, provider: &str, model: &str, effort: &str) -> serde_json::Value {
    json!({"id":id,"provider":provider,"model":model,"effort":effort,"available":true,"capabilities":["code.change"],"description":"test","executable":"test","resource_key":provider})
}
#[test]
fn shortlist_is_bounded_deterministic_and_diverse() {
    let mut list = Vec::new();
    for i in 0..40 {
        list.push(c(
            &format!("c{i}"),
            "cursor",
            &format!("m{i:02}"),
            "default",
        ));
    }
    list.push(c("claude", "claude", "opus", "default"));
    list.push(c("codex", "codex", "user-model", "high"));
    let snap = json!({"candidates":list});
    let task = json!({"requirements":["code.change"],"objective":"implement"});
    let a = shortlist(
        &snap,
        &task,
        "Claude for implementation; Codex for review",
        100,
    )
    .unwrap();
    assert_eq!(a.len(), 16);
    assert_eq!(
        a,
        shortlist(
            &snap,
            &task,
            "Claude for implementation; Codex for review",
            100
        )
        .unwrap()
    );
    assert!(a.iter().any(|x| x["provider"] == "claude"));
    assert!(a.iter().any(|x| x["provider"] == "codex"));
    assert!(a.iter().any(|x| x["provider"] == "cursor"));
}
#[test]
fn exact_pins_never_substitute_and_requirements_filter() {
    let mut unavailable = c("no", "codex", "m", "high");
    unavailable["available"] = json!(false);
    let snap = json!({"candidates":[c("yes","codex","m","high"),c("other","codex","m","low"),unavailable]});
    let pinned = json!({"routing":{"provider":"codex","model":"m","effort":"high"},"requirements":["code.change"]});
    assert_eq!(shortlist(&snap, &pinned, "", 16).unwrap()[0]["id"], "yes");
    assert!(
        shortlist(
            &snap,
            &json!({"routing":{"candidate_id":"missing"}}),
            "",
            16
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        shortlist(&snap, &json!({"routing":{"candidate_id":"no"}}), "", 16)
            .unwrap()
            .is_empty()
    );
    assert!(
        shortlist(&snap, &json!({"requirements":["unknown"]}), "", 16)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn selection_and_archived_workspace_are_checked() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(snapshot(tmp.path(), &json!({"providers":["unknown"]})).is_err());
    let empty = snapshot(tmp.path(), &json!({"providers":[]})).unwrap();
    assert_eq!(empty["schema_version"], 1);
    assert!(empty["candidates"].as_array().unwrap().is_empty());
    std::fs::write(tmp.path().join("ARCHIVED.json"), "{}").unwrap();
    assert!(snapshot(tmp.path(), &json!({})).is_err());
}

#[test]
fn configured_candidate_survives_large_catalog() {
    let mut list = (0..80)
        .map(|i| c(&format!("old{i}"), "codex", &format!("a{i:02}"), "high"))
        .collect::<Vec<_>>();
    let mut configured = c("configured", "codex", "z-current", "high");
    configured["catalog_info"] = json!({"source":"user_config"});
    list.push(configured);
    let result = shortlist(&json!({"candidates":list}), &json!({}), "", 16).unwrap();
    assert_eq!(result[0]["id"], "configured");
    assert_eq!(result.len(), 16);
}
