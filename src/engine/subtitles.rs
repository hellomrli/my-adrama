//! SRT 字幕：按镜头时长累计时间轴，台词即字幕。

use crate::model::Breakdown;

/// 生成 SRT 文本与条数。没有台词的镜头跳过，但时间照常前进。
pub fn srt(bd: &Breakdown) -> (String, usize) {
    let mut out = String::new();
    let mut cursor_ms: u64 = 0;
    let mut n = 0usize;

    for shot in &bd.shots {
        let start = cursor_ms;
        let end = cursor_ms + u64::from(shot.duration_secs) * 1000;
        cursor_ms = end;

        let text = shot.dialogue.trim();
        if text.is_empty() {
            continue;
        }
        n += 1;
        out.push_str(&format!(
            "{n}\n{} --> {}\n{text}\n\n",
            timestamp(start),
            timestamp(end)
        ));
    }
    (out, n)
}

fn timestamp(ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let milli = ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{milli:03}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Shot;

    fn shot(id: &str, dur: u32, dialogue: &str) -> Shot {
        Shot {
            id: id.into(),
            scene_id: "sc".into(),
            number: 1,
            framing: "中景".into(),
            camera: "固定".into(),
            visual: "画面".into(),
            visual_end: String::new(),
            dialogue: dialogue.into(),
            sfx: String::new(),
            duration_secs: dur,
            character_ids: vec![],
            prop_ids: vec![],
            location_id: None,
        }
    }

    #[test]
    fn timeline_accumulates_and_skips_silent_shots() {
        let bd = Breakdown {
            shots: vec![
                shot("a", 5, "你确定这车能上山？"),
                shot("b", 4, ""), // 无台词：时间前进但不出条目
                shot("c", 6, "能上山，也能下山。"),
            ],
            ..Default::default()
        };
        let (text, n) = srt(&bd);
        assert_eq!(n, 2);
        assert!(text.contains("1\n00:00:00,000 --> 00:00:05,000\n你确定这车能上山？"));
        // 第二条从 9 秒开始（5+4），编号连续为 2
        assert!(text.contains("2\n00:00:09,000 --> 00:00:15,000\n能上山，也能下山。"));
    }

    #[test]
    fn timestamp_format_is_srt() {
        assert_eq!(timestamp(0), "00:00:00,000");
        assert_eq!(timestamp(3_661_500), "01:01:01,500");
    }
}
