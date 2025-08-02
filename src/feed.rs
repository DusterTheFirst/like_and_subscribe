use jiff::civil::DateTime;
use monostate::MustBe;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod test {
    use crate::feed::Feed;

    #[test]
    fn parse_sample_file() {
        let sample_video = include_str!("../test_data/sample_video.xml");

        dbg!(quick_xml::de::from_str::<Feed>(sample_video).unwrap());
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Feed {
    #[serde(rename = "@xmlns")]
    _namespace: MustBe!("http://www.w3.org/2005/Atom"),
    #[serde(rename = "@xmlns:yt")]
    _namespace_yt: MustBe!("http://www.youtube.com/xml/schemas/2015"),
    pub entry: Entry,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Entry {
    pub id: String,
    #[serde(rename = "yt:videoId")]
    #[serde(alias = "videoId")] // quick_xml ignores namespace prefixes with serde
    pub video_id: String,
    #[serde(rename = "yt:channelId")]
    #[serde(alias = "channelId")] // quick_xml ignores namespace prefixes with serde
    pub channel_id: String,
    pub title: String,
    pub published: DateTime,
    pub updated: DateTime,
}
