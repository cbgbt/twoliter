//! Provides a utility for merging toml values
pub(crate) trait Merge {
    fn merge(&mut self, other: &Self);
}

impl Merge for toml::Value {
    fn merge(&mut self, other: &Self) {
        match (self, other) {
            (toml::Value::Table(a), toml::Value::Table(b)) => a.merge(b),
            (a, b) => {
                *a = b.clone();
            }
        }
    }
}

impl Merge for toml::Table {
    fn merge(&mut self, other: &Self) {
        for (k, v) in other {
            if let Some(a_val) = self.get_mut(k) {
                a_val.merge(v);
            } else {
                self.insert(k.clone(), v.clone());
            }
        }
    }
}
