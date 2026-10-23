#![allow(dead_code)]
use serde_json::{Value, json};
use switchbite::{candidate_key, contracts::MODEL};
pub fn candidate(id: &str) -> Value {
    json!({"id":id,"description":"Translates customer correspondence.","capabilities":["translate","crm.read"],"available":true,"extensions":{"host_handle":"opaque:42"}})
}
pub fn request() -> Value {
    json!({"version":1,"task":{"id":"ticket:42","objective":"Translate a Korean customer reply into English.","requirements":["translate","crm.read"],"context_refs":["crm:42/7"],"extensions":{"locale":"en"}},"candidates":[candidate("translator:ko/en")],"preferences":"Preserve meaning and tone."})
}
pub fn node(id: &str, deps: &[&str]) -> Value {
    json!({"id":id,"objective":"Translate reply","depends_on":deps,"requirements":["translate"],"acceptance":["Meaning preserved"],"outputs":[format!("artifact:{id}")]})
}
pub fn feedback() -> Value {
    json!({"version":1,"task_id":"ticket:42","candidate_id":"translator:ko/en","outcome":"succeeded","evidence":["receipt:host/42"]})
}
pub fn score(p: &[f64]) -> Value {
    let mode = (0..p.len())
        .max_by(|a, b| p[*a].total_cmp(&p[*b]).then_with(|| b.cmp(a)))
        .unwrap();
    let spread: f64 = p
        .iter()
        .enumerate()
        .map(|(i, p)| *p * (i as f64 - mode as f64).abs())
        .sum();
    let even = (0..p.len())
        .map(|i| (i as f64 - (p.len() - 1) as f64 / 2.0).abs())
        .sum::<f64>()
        / p.len() as f64;
    let probabilities: serde_json::Map<_, _> = p
        .iter()
        .enumerate()
        .map(|(i, p)| (i.to_string(), json!(p)))
        .collect();
    json!({"type":"score","score":p.iter().enumerate().map(|(i,p)| i as f64*p).sum::<f64>(),"confidence":(1.0-spread/even).max(0.0),"probabilities":probabilities})
}
pub fn judgment(body: &Value) -> Value {
    let mut answers = json!({"context_sufficient":{"type":"noul","noul":1.0}});
    for (key, _) in body["state"]["candidates"].as_object().unwrap() {
        answers[format!("fit_{key}")] = score(&[0.0, 0.0, 0.0, 1.0]);
        answers[format!("preference_{key}")] = score(&[0.0, 0.0, 1.0]);
    }
    json!({"model":MODEL,"answers":answers})
}
pub fn replay() -> Value {
    let mut r = request();
    r["judgment"] = judgment(&switchbite::judgment_body(&r).unwrap());
    r
}
pub fn fit_key(id: &str) -> String {
    format!("fit_{}", candidate_key(id))
}
pub fn preference_key(id: &str) -> String {
    format!("preference_{}", candidate_key(id))
}
