//! Analytics for your own posts, trend reports and the competitor watchlist.
//! Everything here comes from the YouTube Data API using your signed-in account;
//! nothing is scraped and no one else's videos are downloaded.

use crate::{db::Db, jobs::now_str, oauth};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

async fn yt(path: &str, query: &[(&str, &str)]) -> Result<Value> {
    let token = oauth::access_token(oauth::provider("youtube")?).await?;
    let resp = reqwest::Client::new().get(format!("https://www.googleapis.com/youtube/v3/{path}")).query(query).bearer_auth(token).send().await?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("YouTube API {path}: {} {}", status, v["error"]["message"].as_str().unwrap_or(""));
    }
    Ok(v)
}

/// ISO-8601 duration (PT1M5S) to seconds.
pub fn iso_secs(d: &str) -> f64 {
    let mut total = 0.0;
    let mut num = String::new();
    for c in d.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' => { total += num.parse::<f64>().unwrap_or(0.0) * 3600.0; num.clear() }
            'M' => { total += num.parse::<f64>().unwrap_or(0.0) * 60.0; num.clear() }
            'S' => { total += num.parse::<f64>().unwrap_or(0.0); num.clear() }
            _ => num.clear(),
        }
    }
    total
}

async fn videos(ids: &[String]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    for batch in ids.chunks(50) {
        let v = yt("videos", &[("part", "snippet,statistics,contentDetails"), ("id", &batch.join(","))]).await?;
        out.extend(v["items"].as_array().cloned().unwrap_or_default());
    }
    Ok(out)
}

fn n(v: &Value) -> i64 {
    v.as_str().and_then(|s| s.parse().ok()).or(v.as_i64()).unwrap_or(0)
}

// ---------- your analytics ----------

/// Snapshot of views/likes/comments for every published YouTube post.
pub async fn refresh_stats(db: &Db) -> Result<String> {
    let posts: Vec<(i64, String)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id, remote_id FROM posts WHERE platform='youtube' AND status='published' AND remote_id IS NOT NULL")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if posts.is_empty() {
        return Ok("No published YouTube posts yet".into());
    }
    let ids: Vec<String> = posts.iter().map(|p| p.1.clone()).collect();
    let items = videos(&ids).await?;
    let ts = now_str();
    let conn = db.conn();
    for it in &items {
        let id = it["id"].as_str().unwrap_or("");
        if let Some((post_id, _)) = posts.iter().find(|p| p.1 == id) {
            let s = &it["statistics"];
            conn.execute(
                "INSERT OR REPLACE INTO post_stats(post_id,ts,views,likes,comments,shares) VALUES(?1,?2,?3,?4,?5,NULL)",
                rusqlite::params![post_id, ts, n(&s["viewCount"]), n(&s["likeCount"]), n(&s["commentCount"])],
            )?;
        }
    }
    Ok(format!("Updated stats for {} video(s)", items.len()))
}

/// Average latest views per clip category, used to nudge clip scoring toward what works.
/// Needs at least 3 published clips in a category before it counts.
pub fn category_performance(db: &Db) -> Result<HashMap<String, (f64, usize)>> {
    let conn = db.conn();
    let mut st = conn.prepare(
        "SELECT c.category, s.views FROM posts p JOIN clips c ON c.id=p.clip_id
         JOIN post_stats s ON s.post_id=p.id
         WHERE s.ts=(SELECT MAX(ts) FROM post_stats WHERE post_id=p.id)",
    )?;
    let mut agg: HashMap<String, (f64, usize)> = HashMap::new();
    for row in st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)))? {
        let (cat, views) = row?;
        let e = agg.entry(cat.unwrap_or_else(|| "moment".into())).or_insert((0.0, 0));
        e.0 += views.unwrap_or(0) as f64;
        e.1 += 1;
    }
    Ok(agg.into_iter().map(|(k, (sum, cnt))| (k, (sum / cnt as f64, cnt))).collect())
}

/// Multiplier per category: 0.85–1.2 relative to the overall average, once there is data.
pub fn category_boost(perf: &HashMap<String, (f64, usize)>, category: &str) -> f64 {
    let qualified: Vec<f64> = perf.values().filter(|(_, c)| *c >= 3).map(|(v, _)| *v).collect();
    if qualified.len() < 2 {
        return 1.0;
    }
    let overall = qualified.iter().sum::<f64>() / qualified.len() as f64;
    match perf.get(category) {
        Some((avg, cnt)) if *cnt >= 3 && overall > 0.0 => (avg / overall).clamp(0.85, 1.2),
        _ => 1.0,
    }
}

// ---------- trend report ----------

fn title_traits(titles: &[String]) -> Value {
    let k = titles.len().max(1) as f64;
    let caps = titles.iter().filter(|t| t.split_whitespace().any(|w| w.len() > 2 && w.chars().all(|c| !c.is_lowercase()))).count();
    let question = titles.iter().filter(|t| t.contains('?')).count();
    let number = titles.iter().filter(|t| t.chars().any(|c| c.is_ascii_digit())).count();
    let emoji = titles.iter().filter(|t| t.chars().any(|c| (c as u32) > 0x2600)).count();
    let avg_len = titles.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / k;
    json!({
        "avg_title_chars": avg_len.round(),
        "pct_with_caps_word": (caps as f64 / k * 100.0).round(),
        "pct_questions": (question as f64 / k * 100.0).round(),
        "pct_numbers": (number as f64 / k * 100.0).round(),
        "pct_emoji": (emoji as f64 / k * 100.0).round(),
    })
}

fn top_terms(texts: &[String], hashtags: bool, k: usize) -> Vec<(String, usize)> {
    let stop = ["the", "and", "for", "you", "with", "this", "que", "los", "las", "por", "con", "una", "del", "para", "shorts", "short"];
    let mut m: HashMap<String, usize> = HashMap::new();
    for t in texts {
        for w in t.split_whitespace() {
            let is_tag = w.starts_with('#');
            if hashtags != is_tag {
                continue;
            }
            let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            if w.chars().count() < 3 || stop.contains(&w.as_str()) {
                continue;
            }
            *m.entry(w).or_default() += 1;
        }
    }
    let mut v: Vec<_> = m.into_iter().filter(|(_, c)| *c >= 2).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(k);
    v
}

/// Searches the last 7 days of YouTube for each keyword (most viewed first) and summarises
/// what the top videos have in common. Costs ~100 API quota units per keyword.
pub async fn trend_report(db: &Db, keywords: &[String]) -> Result<String> {
    if keywords.is_empty() {
        bail!("add trend keywords in Trend Research first");
    }
    let since = (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut ids = Vec::new();
    for kw in keywords {
        let v = yt("search", &[("part", "id"), ("q", kw), ("type", "video"), ("order", "viewCount"), ("publishedAfter", &since), ("maxResults", "25")]).await?;
        ids.extend(v["items"].as_array().into_iter().flatten().filter_map(|i| i["id"]["videoId"].as_str().map(String::from)));
    }
    ids.sort();
    ids.dedup();
    let items = videos(&ids).await?;
    if items.is_empty() {
        bail!("YouTube returned no videos for these keywords this week");
    }
    let mut rows: Vec<Value> = items
        .iter()
        .map(|it| {
            let s = &it["snippet"];
            json!({
                "id": it["id"], "title": s["title"], "channel": s["channelTitle"], "published": s["publishedAt"],
                "thumb": s["thumbnails"]["high"]["url"], "views": n(&it["statistics"]["viewCount"]),
                "likes": n(&it["statistics"]["likeCount"]), "comments": n(&it["statistics"]["commentCount"]),
                "seconds": iso_secs(it["contentDetails"]["duration"].as_str().unwrap_or("")),
            })
        })
        .collect();
    rows.sort_by_key(|r| -r["views"].as_i64().unwrap_or(0));
    let titles: Vec<String> = rows.iter().filter_map(|r| r["title"].as_str().map(String::from)).collect();
    let descs: Vec<String> = items.iter().filter_map(|i| i["snippet"]["description"].as_str().map(String::from)).chain(titles.iter().cloned()).collect();
    let short = rows.iter().filter(|r| r["seconds"].as_f64().unwrap_or(0.0) <= 180.0).count();
    let mut hours = [0usize; 24];
    for r in &rows {
        if let Some(h) = r["published"].as_str().and_then(|p| p.get(11..13)).and_then(|h| h.parse::<usize>().ok()) {
            hours[h % 24] += 1;
        }
    }
    let best_hour = hours.iter().enumerate().max_by_key(|(_, c)| **c).map(|(h, _)| h).unwrap_or(0);
    let median_views = rows.get(rows.len() / 2).and_then(|r| r["views"].as_i64()).unwrap_or(0);
    let engagement: f64 = rows.iter().map(|r| (r["likes"].as_f64().unwrap_or(0.0) + r["comments"].as_f64().unwrap_or(0.0)) / r["views"].as_f64().unwrap_or(1.0).max(1.0)).sum::<f64>() / rows.len() as f64;
    let report = json!({
        "videos_analyzed": rows.len(),
        "median_views": median_views,
        "avg_engagement_pct": (engagement * 1000.0).round() / 10.0,
        "pct_short_form": (short as f64 / rows.len() as f64 * 100.0).round(),
        "top_publish_hour_utc": best_hour,
        "title_traits": title_traits(&titles),
        "top_words": top_terms(&titles, false, 15),
        "top_hashtags": top_terms(&descs, true, 15),
        "top_videos": rows.iter().take(12).collect::<Vec<_>>(),
    });
    db.conn().execute("INSERT INTO trend_reports(keywords, report) VALUES(?1, ?2)", rusqlite::params![keywords.join(", "), report.to_string()])?;
    Ok(format!("Trend report from {} videos", rows.len()))
}

/// Short text summary of the latest report, fed to clip scoring and metadata.
pub fn latest_trend_notes(db: &Db) -> String {
    let r: Option<String> = db.conn().query_row("SELECT report FROM trend_reports ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).ok();
    let Some(v) = r.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return String::new() };
    let words: Vec<String> = v["top_words"].as_array().into_iter().flatten().filter_map(|p| p[0].as_str().map(String::from)).take(10).collect();
    let tags: Vec<String> = v["top_hashtags"].as_array().into_iter().flatten().filter_map(|p| p[0].as_str().map(|t| format!("#{t}"))).take(8).collect();
    format!("Frequent title words: {}. Popular hashtags: {}. {}% of top videos are short-form.", words.join(", "), tags.join(" "), v["pct_short_form"])
}

// ---------- watchlist ----------

pub async fn refresh_watchlist(db: &Db) -> Result<String> {
    let entries: Vec<(i64, String)> = {
        let conn = db.conn();
        let mut st = conn.prepare("SELECT id, handle FROM watchlist")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut ok = 0;
    for (id, handle) in entries {
        match watch_one(&handle).await {
            Ok((channel_id, title, subs, summary)) => {
                db.conn().execute(
                    "UPDATE watchlist SET channel_id=?2, title=?3, subscribers=?4, summary=?5, checked_at=?6, error=NULL WHERE id=?1",
                    rusqlite::params![id, channel_id, title, subs, summary.to_string(), now_str()],
                )?;
                ok += 1;
            }
            Err(e) => {
                db.conn().execute("UPDATE watchlist SET checked_at=?2, error=?3 WHERE id=?1", rusqlite::params![id, now_str(), format!("{e:#}")])?;
            }
        }
    }
    Ok(format!("Refreshed {ok} creator(s)"))
}

async fn watch_one(handle: &str) -> Result<(String, String, i64, Value)> {
    let h = handle.trim();
    let q: (&str, String) = if h.starts_with("UC") && h.len() == 24 { ("id", h.into()) } else { ("forHandle", format!("@{}", h.trim_start_matches('@'))) };
    let ch = yt("channels", &[("part", "snippet,statistics,contentDetails"), (q.0, &q.1)]).await?;
    let c = ch["items"].get(0).ok_or_else(|| anyhow!("no YouTube channel found for {h}"))?;
    let uploads = c["contentDetails"]["relatedPlaylists"]["uploads"].as_str().ok_or_else(|| anyhow!("channel has no uploads list"))?;
    let pl = yt("playlistItems", &[("part", "contentDetails"), ("playlistId", uploads), ("maxResults", "15")]).await?;
    let ids: Vec<String> = pl["items"].as_array().into_iter().flatten().filter_map(|i| i["contentDetails"]["videoId"].as_str().map(String::from)).collect();
    let vids = videos(&ids).await?;
    let recent: Vec<Value> = vids
        .iter()
        .map(|v| json!({
            "id": v["id"], "title": v["snippet"]["title"], "published": v["snippet"]["publishedAt"],
            "thumb": v["snippet"]["thumbnails"]["medium"]["url"], "views": n(&v["statistics"]["viewCount"]),
            "seconds": iso_secs(v["contentDetails"]["duration"].as_str().unwrap_or("")),
        }))
        .collect();
    let dates: Vec<chrono::DateTime<chrono::Utc>> = recent.iter().filter_map(|r| r["published"].as_str()?.parse().ok()).collect();
    let per_week = match (dates.iter().max(), dates.iter().min()) {
        (Some(a), Some(b)) if a > b => dates.len() as f64 / ((*a - *b).num_hours() as f64 / 168.0).max(1.0),
        _ => dates.len() as f64,
    };
    let views: Vec<i64> = recent.iter().map(|r| r["views"].as_i64().unwrap_or(0)).collect();
    let avg = if views.is_empty() { 0 } else { views.iter().sum::<i64>() / views.len() as i64 };
    let shorts = recent.iter().filter(|r| r["seconds"].as_f64().unwrap_or(0.0) <= 180.0).count();
    let titles: Vec<String> = recent.iter().filter_map(|r| r["title"].as_str().map(String::from)).collect();
    // Breakout: a recent video with 3× the creator's average.
    let breakout = recent.iter().find(|r| avg > 0 && r["views"].as_i64().unwrap_or(0) > avg * 3).cloned();
    let summary = json!({
        "posts_per_week": (per_week * 10.0).round() / 10.0,
        "avg_views": avg,
        "pct_shorts": if recent.is_empty() { 0.0 } else { (shorts as f64 / recent.len() as f64 * 100.0).round() },
        "title_traits": title_traits(&titles),
        "breakout": breakout,
        "recent": recent,
    });
    Ok((
        c["id"].as_str().unwrap_or("").into(),
        c["snippet"]["title"].as_str().unwrap_or(h).into(),
        n(&c["statistics"]["subscriberCount"]),
        summary,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_durations() {
        assert_eq!(iso_secs("PT1M5S"), 65.0);
        assert_eq!(iso_secs("PT2H"), 7200.0);
        assert_eq!(iso_secs("PT45S"), 45.0);
    }

    #[test]
    fn category_boost_needs_data_and_is_bounded() {
        let mut p = HashMap::new();
        assert_eq!(category_boost(&p, "funny"), 1.0);
        p.insert("funny".into(), (3000.0, 4));
        p.insert("gameplay".into(), (1000.0, 5));
        assert!((category_boost(&p, "funny") - 1.2).abs() < 1e-9);
        assert!((category_boost(&p, "gameplay") - 0.85).abs() < 1e-9);
        assert_eq!(category_boost(&p, "debate"), 1.0);
    }

    #[test]
    fn hashtags_and_words_are_counted_separately() {
        let t = vec!["Clip loco #valorant #clips".to_string(), "Otro clip #valorant".to_string(), "clip".into()];
        assert_eq!(top_terms(&t, true, 5), vec![("valorant".to_string(), 2)]);
        assert_eq!(top_terms(&t, false, 5), vec![("clip".to_string(), 3)]);
    }
}
