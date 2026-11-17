import unittest
from coverage_source import excluded_test_lines


class CoverageSourceTests(unittest.TestCase):
    def test_inline_module_does_not_hide_later_production(self):
        source = '#[cfg(test)]\nmod tests { fn fake() { } }\nfn real() { }\n'
        self.assertEqual(excluded_test_lines(source), {1, 2})

    def test_feature_or_test_helpers_depend_on_build_mode(self):
        source = 'impl A {\n#[cfg(any(test, feature = "extended-workflows"))]\nfn helper() { let x = "}"; }\nfn real() { }\n}\n'
        self.assertEqual(excluded_test_lines(source), {2, 3})
        self.assertEqual(excluded_test_lines(source, extended=True), set())

    def test_raw_strings_chars_comments_and_lifetimes_keep_balanced_items(self):
        source = '''#[cfg(test)]
fn helper<'a>() {
    let a = r###"} /* } */"###;
    let b = '}'; /* nested /* } */ } */
    // } }
}
fn real() {}
'''
        self.assertEqual(excluded_test_lines(source), set(range(1, 7)))

    def test_default_only_commands_are_test_helpers_in_extended_mode(self):
        source = 'impl A {\n#[cfg(any(test, not(feature = "extended-workflows")))]\nfn dispatch() { let x = "}"; }\nfn validate() { }\n}\n'
        self.assertEqual(excluded_test_lines(source), set())
        self.assertEqual(excluded_test_lines(source, extended=True), {2, 3})

    def test_field_and_module_declarations_do_not_hide_surroundings(self):
        source = 'enum X {\n#[cfg(any(test, feature = "extended-workflows"))]\nExtra,\nCore,\n}\n#[cfg(test)]\n#[path = "other.rs"]\nmod tests;\nfn real() {}'
        self.assertEqual(excluded_test_lines(source), {2, 3, 6, 7, 8})

    def test_cfg_text_inside_comments_is_not_an_attribute(self):
        self.assertEqual(excluded_test_lines('// #[cfg(test)]\nfn real() {}'), set())


if __name__ == '__main__':
    unittest.main()
