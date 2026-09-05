import importlib.util
import io
import sys
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("trend_tui.py")
SPEC = importlib.util.spec_from_file_location("trend_tui", MODULE_PATH)
trend_tui = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = trend_tui
SPEC.loader.exec_module(trend_tui)


class CompatibilityTests(unittest.TestCase):
    def test_catalog_uses_existing_video_and_thumbnail_fixtures(self):
        self.assertEqual(len(trend_tui.VIDEO_ENTRIES), 4)
        for entry in trend_tui.VIDEO_ENTRIES:
            self.assertEqual(entry.video.suffix.lower(), ".mp4")
            self.assertTrue(entry.video.is_file())

    def test_video_catalog_forms_a_bounded_two_column_grid(self):
        layout = trend_tui.calculate_layout(80, 24)
        cards = trend_tui.grid_cards(layout)
        self.assertEqual(len(cards), 4)
        for card in cards:
            self.assertGreater(card.width, 0)
            self.assertLessEqual(card.column + card.width, layout.columns)
            self.assertLess(card.row + card.thumbnail_height + 2, layout.rows - 1)

    def test_arrow_navigation_is_two_dimensional_and_wraps(self):
        self.assertEqual(trend_tui.move_selection(0, "right"), 1)
        self.assertEqual(trend_tui.move_selection(1, "down"), 3)
        self.assertEqual(trend_tui.move_selection(3, "left"), 2)
        self.assertEqual(trend_tui.move_selection(2, "up"), 0)
        self.assertEqual(trend_tui.move_selection(0, "left"), 1)

    def test_card_and_fullscreen_regions_remain_application_owned(self):
        layout = trend_tui.calculate_layout(80, 24)
        card = trend_tui.card_layout(layout, 3)
        fullscreen = trend_tui.fullscreen_layout(layout)
        self.assertGreater(card.preview_row, 4)
        self.assertGreater(card.preview_column, layout.columns // 2)
        self.assertLessEqual(card.preview_column + card.preview_width, layout.columns)
        self.assertEqual((fullscreen.preview_row, fullscreen.preview_column), (2, 1))
        self.assertEqual(fullscreen.preview_width, layout.columns)

    def test_card_scale_resizes_borders_and_preview_region(self):
        layout = trend_tui.calculate_layout(80, 24)
        full = trend_tui.card_layout(layout, 0, 100)
        half = trend_tui.card_layout(layout, 0, 50)
        self.assertLess(half.preview_width, full.preview_width)
        self.assertLess(half.preview_height, full.preview_height)
        self.assertGreater(half.preview_column, full.preview_column)
        self.assertEqual(half.preview_row, full.preview_row)

    def test_card_scale_controls_are_clamped(self):
        self.assertEqual(trend_tui.resize_card_scale(100, True), 100)
        self.assertEqual(trend_tui.resize_card_scale(100, False), 75)
        self.assertEqual(trend_tui.resize_card_scale(75, False), 50)
        self.assertEqual(trend_tui.resize_card_scale(50, False), 50)

    def test_thumbnail_ansi_cache_is_scoped_to_its_terminal_surface(self):
        layout = trend_tui.calculate_layout(80, 24)
        capabilities = trend_tui.Capabilities("truecolor", "unicode")
        key = trend_tui.thumbnail_surface_key(layout, capabilities, 75)

        self.assertEqual(key, trend_tui.thumbnail_surface_key(layout, capabilities, 75))
        self.assertNotEqual(
            key,
            trend_tui.thumbnail_surface_key(
                trend_tui.calculate_layout(120, 40), capabilities, 75
            ),
        )
        self.assertNotEqual(key, trend_tui.thumbnail_surface_key(layout, capabilities, 50))
        self.assertNotEqual(
            key,
            trend_tui.thumbnail_surface_key(
                layout, trend_tui.Capabilities("16", "ascii"), 75
            ),
        )

    def test_required_terminal_sizes_are_bounded(self):
        compact = trend_tui.calculate_layout(40, 12)
        self.assertTrue(compact.compact)
        for columns, rows in ((80, 24), (120, 40), (240, 80)):
            layout = trend_tui.calculate_layout(columns, rows)
            self.assertFalse(layout.compact)
            self.assertGreater(layout.preview_width, 0)
            self.assertGreater(layout.preview_height, 0)
            if columns >= 72:
                self.assertLess(2 + layout.preview_column - 4, layout.preview_column)
            self.assertLessEqual(layout.preview_column + layout.preview_width, columns)
            self.assertLessEqual(layout.preview_row + layout.preview_height, rows)

    def test_very_small_terminal_uses_compact_fallback(self):
        self.assertTrue(trend_tui.calculate_layout(39, 11).compact)

    def test_detection_is_conservative_without_terminal_evidence(self):
        capabilities = trend_tui.detect_capabilities("auto", "auto", {})
        self.assertEqual(capabilities.color, "mono")
        self.assertIn(capabilities.glyphs, ("ascii", "unicode"))

    def test_explicit_capabilities_override_environment(self):
        capabilities = trend_tui.detect_capabilities(
            "256", "ascii", {"TERM": "dumb", "NO_COLOR": "1"}
        )
        self.assertEqual(capabilities, trend_tui.Capabilities("256", "ascii"))

    def test_application_text_establishes_default_foreground_and_background(self):
        original_stdout = trend_tui.sys.stdout
        output = io.StringIO()
        trend_tui.sys.stdout = output
        try:
            trend_tui.write_text("UI")
        finally:
            trend_tui.sys.stdout = original_stdout
        self.assertEqual(output.getvalue(), "\x1b[0;39;49mUI")

    def test_seconds_are_encoded_as_codec_neutral_nanoseconds(self):
        self.assertEqual(trend_tui.seconds_to_nanoseconds(83.25), 83_250_000_000)
        self.assertEqual(trend_tui.optional_nanoseconds("-1"), None)
        self.assertEqual(trend_tui.optional_nanoseconds("50000000"), 50_000_000)
        with self.assertRaises(ValueError):
            trend_tui.seconds_to_nanoseconds(-0.1)

    def test_frame_unavailable_is_distinct_from_protocol_failure(self):
        self.assertTrue(issubclass(trend_tui.FrameUnavailable, RuntimeError))


if __name__ == "__main__":
    unittest.main()