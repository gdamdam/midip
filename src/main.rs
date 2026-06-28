fn main() {
    // Phase 1 scaffold: the binary entry point is wired up incrementally as
    // modules land (engine, ui, app). For now it is intentionally empty so the
    // crate builds and the test suite is green from the very first commit.
}

#[cfg(test)]
mod tests {
    #[test]
    fn scaffold_builds() {
        // Sanity check that the test harness runs at all.
        assert_eq!(2 + 2, 4);
    }
}
