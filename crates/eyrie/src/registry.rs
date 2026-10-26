//! Les modules connus de la suite, dans l'ordre de la barre d'activité. Ajouter un module : une
//! crate qui expose sa `factory()`, une ligne ici ; rien à retoucher ailleurs.

use suite_sdk::ModuleFactory;

pub fn factories() -> Vec<ModuleFactory> {
    vec![
        suite_fawkes::suite::factory(),
        suite_dragontree::factory(),
        suite_owlery::factory(),
        suite_patronus::factory(),
        suite_menagerie::factory(),
        suite_grimoire::factory(),
        suite_gringotts::factory(),
        suite_carnet::factory(),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_ids_are_unique_and_stable() {
        let ids: Vec<&str> = super::factories().iter().map(|f| f.info.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "{ids:?}");
        for id in ids {
            assert!(id.chars().all(|c| c.is_ascii_lowercase()), "{id}");
        }
    }
}
