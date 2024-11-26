#[cfg_attr(test, derive(Debug))]
pub(crate) struct Diff<T> {
    pub(crate) new: Vec<T>,
    pub(crate) removed: fxhash::FxHashSet<T>,
}

impl<T> Diff<T> {
    pub(crate) fn make<'a, I>(old: fxhash::FxHashSet<T>, new: I) -> Self
    where
        T: std::hash::Hash + Eq + Clone + 'a,
        I: IntoIterator<Item = &'a T>,
    {
        let mut new_items = Vec::new();
        let mut removed = old;
        // Iterate over `new` version, removing each item from the
        // `removed` set. Thus, the `removed` set in the end will
        // contain items which are present in `old`, but not present
        // in `new` - those elements are removed.
        for item in new {
            // If `removed` set previously didn't contain the item, then
            // it's new.
            let had = removed.remove(item);
            if !had {
                new_items.push(item.clone());
            }
        }

        Self {
            new: new_items,
            removed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl<T: Eq + Ord + Clone> PartialEq for Diff<T> {
        fn eq(&self, other: &Self) -> bool {
            let mut lhs_new = self.new.clone();
            let mut lhs_removed = self.removed.iter().cloned().collect::<Vec<_>>();

            lhs_new.sort();
            lhs_removed.sort();

            let mut rhs_new = other.new.clone();
            let mut rhs_removed = other.removed.iter().cloned().collect::<Vec<_>>();

            rhs_new.sort();
            rhs_removed.sort();

            (lhs_new == rhs_new) && (lhs_removed == rhs_removed)
        }
    }
    impl<T: Eq + Ord + Clone> Eq for Diff<T> {}

    #[test]
    fn diff_works() {
        struct Input {
            old: Vec<u64>,
            new: Vec<u64>,
            expected: Diff<u64>,
        }

        impl Input {
            fn new(
                old: impl AsRef<[u64]>,
                new: impl AsRef<[u64]>,
                expected: (impl AsRef<[u64]>, impl AsRef<[u64]>),
            ) -> Self {
                let old = old.as_ref();
                let new = new.as_ref();
                let (diff_new, diff_removed) = (expected.0.as_ref(), expected.1.as_ref());

                Self {
                    old: old.to_vec(),
                    new: new.to_vec(),
                    expected: Diff {
                        new: diff_new.to_vec(),
                        removed: diff_removed.iter().copied().collect(),
                    },
                }
            }
        }

        let inputs = [
            Input::new([], [], ([], [])),
            Input::new([1, 2, 3], [1, 2, 3], ([], [])),
            Input::new([1, 2, 3], [1, 3], ([], [2])),
            Input::new([1, 2, 3], [1, 2, 3, 4], ([4], [])),
            Input::new([1, 2, 3], [1, 3, 4], ([4], [2])),
        ];
        for Input { new, old, expected } in inputs {
            let actual = Diff::make(old.into_iter().collect(), new.iter());

            assert_eq!(expected, actual);
        }
    }
}
