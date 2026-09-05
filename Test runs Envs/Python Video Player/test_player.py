import importlib.util
import sys
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import Mock, patch


MODULE_PATH = Path(__file__).with_name("player.py")
SPEC = importlib.util.spec_from_file_location("trend_video_player", MODULE_PATH)
player = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = player
SPEC.loader.exec_module(player)


class InputTests(unittest.TestCase):
    def test_keyboard_controls(self):
        expected = {
            " ": "toggle",
            "q": "quit",
            "n": "next",
            "t": "volume_down",
            "Y": "volume_up",
            "R": "restart",
            "f": "fullscreen",
            "1": "quality_fast",
            "2": "quality_balanced",
            "3": "quality_quality",
            "c": "color",
            "g": "glyph",
            "\x1b[D": "seek_back",
            "\x1b[C": "seek_forward",
            "\x1b[1;2D": "seek_large_back",
            "\x1b[1;2C": "seek_large_forward",
        }
        for sequence, action in expected.items():
            with self.subTest(sequence=sequence):
                self.assertEqual(player.parse_input(sequence), action)

    def test_sgr_mouse_click(self):
        self.assertEqual(player.parse_input("\x1b[<0;42;19M"), ("mouse", 42, 19))
        self.assertIsNone(player.parse_input("x"))


class LayoutTests(unittest.TestCase):
    def test_required_terminal_sizes_are_bounded(self):
        for columns, rows in ((40, 12), (80, 24), (120, 40)):
            with self.subTest(columns=columns, rows=rows):
                layout = player.calculate_layout(columns, rows, False)
                video = layout.video
                self.assertGreater(video.preview_width, 0)
                self.assertGreater(video.preview_height, 0)
                self.assertLessEqual(video.preview_column + video.preview_width, columns)
                self.assertLessEqual(video.preview_row + video.preview_height, layout.progress_row)
                self.assertLess(layout.progress_row, layout.controls_row)
                self.assertLess(layout.controls_row, layout.status_row)
                self.assertLess(layout.status_row, layout.debug_row)

    def test_fullscreen_reuses_application_owned_rectangle(self):
        layout = player.calculate_layout(120, 40, True)
        self.assertEqual((layout.video.preview_row, layout.video.preview_column), (2, 2))
        self.assertEqual(layout.video.preview_width, 118)
        self.assertEqual(layout.progress_row, 40)
        self.assertIsNone(layout.controls_row)


class ProgressTests(unittest.TestCase):
    def test_bar_coordinates_are_one_based_and_seek_is_clamped(self):
        line, start, width = player.progress_bar(50.0, 100.0, 80)
        self.assertIn(line[start - 1], "=o-")
        self.assertEqual(player.progress_seek(start, start, width, 100.0), 0.0)
        self.assertEqual(player.progress_seek(start + width - 1, start, width, 100.0), 100.0)
        self.assertEqual(player.progress_seek(start - 100, start, width, 100.0), 0.0)
        self.assertEqual(player.progress_seek(start + width + 100, start, width, 100.0), 100.0)

    def test_progress_and_time_are_bounded(self):
        line, _, _ = player.progress_bar(200.0, 100.0, 40)
        self.assertEqual(len(line), 40)
        self.assertIn("100.0%", line)
        self.assertEqual(player.format_time(-1), "00:00")
        self.assertEqual(player.format_time(83.033, True), "01:23.033")


class AudioFallbackTests(unittest.TestCase):
    def test_volume_is_clamped(self):
        audio = player.AudioProvider(50)
        audio.set_volume(-5)
        self.assertEqual(audio.volume, 0)
        audio.set_volume(105)
        self.assertEqual(audio.volume, 100)

    def test_no_audio_provider_preserves_master_clock_position(self):
        audio = player.AudioProvider()
        audio.start(0.0)
        audio.pause()
        audio.resume(5.0)
        audio.seek(8.0, True)
        self.assertEqual(audio.reported_position(12.5), 12.5)
        self.assertIn("PlaybackClock", audio.detail)
        audio.stop()

    def test_ffplay_pause_stops_and_resume_restarts_at_master_position(self):
        audio = player.FfplayAudioProvider("ffplay", Path("video.mp4"))
        process = Mock()
        process.poll.return_value = None
        audio.process = process

        audio.pause()

        process.terminate.assert_called_once_with()
        process.wait.assert_called_once_with(timeout=1)
        self.assertIsNone(audio.process)
        with patch.object(audio, "start") as start:
            audio.resume(12.5)
        start.assert_called_once_with(12.5)

    def test_ffplay_launch_receives_current_volume(self):
        audio = player.FfplayAudioProvider("ffplay", Path("video.mp4"))
        audio.set_volume(65)
        with patch.object(player.subprocess, "Popen") as popen:
            audio.start(12.5)

        command = popen.call_args.args[0]
        self.assertEqual(command[command.index("-volume") + 1], "65")


class PlaylistTests(unittest.TestCase):
    def test_explicit_playlist_preserves_order(self):
        videos = [Path("first.mp4"), Path("second.mp4")]
        self.assertEqual(player.playlist_from_args(videos), tuple(videos))

    def test_default_playlist_has_multiple_existing_videos(self):
        playlist = player.playlist_from_args([])
        self.assertGreater(len(playlist), 1)
        self.assertTrue(all(path.is_file() for path in playlist))

    def test_next_advances_in_order_and_wraps(self):
        videos = [Path("first.mp4"), Path("second.mp4")]
        arguments = Namespace(videos=videos)
        with patch.object(
            player, "run_video", side_effect=(player.NEXT_VIDEO, player.NEXT_VIDEO, 0)
        ) as run_video:
            self.assertEqual(player.run_player(arguments), 0)

        self.assertEqual(
            [call.args[1] for call in run_video.call_args_list],
            [videos[0], videos[1], videos[0]],
        )


if __name__ == "__main__":
    unittest.main()
