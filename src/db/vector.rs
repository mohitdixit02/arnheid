//! CockroachDB's native `VECTOR` type has no client-side binary codec in
//! sqlx (unlike the pgvector Postgres extension's registered OID), so
//! vectors cross the wire as bracketed text literals: `encode` for binds
//! (works directly against a `VECTOR(n)` column or in a `<=>` comparison,
//! no cast needed), `decode` for reads (pair with a `col::text` cast in
//! the SELECT — CockroachDB won't decode `vector` straight into `FLOAT4[]`).

pub fn encode(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, f) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&f.to_string());
    }
    s.push(']');
    s
}

pub fn decode(s: &str) -> Vec<f32> {
    s.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let v = vec![0.5_f32, -1.25, 3.0];
        assert_eq!(decode(&encode(&v)), v);
    }

    #[test]
    fn decode_empty() {
        assert!(decode("[]").is_empty());
    }
}
