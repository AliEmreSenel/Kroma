//! Import panel — Shadertoy URL/file import.

use iced::widget::{column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use crate::Message;
use crate::panels::dashboard::{btn_primary, btn_secondary, card};
use crate::panels::{AppContext, Panel};

pub struct ImportPanel;

impl ImportPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for ImportPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let local_card = card(
            "Import Local File",
            column![
                text("Convert a Shadertoy GLSL file into a .shade package.")
                    .size(11),
                row![
                    text_input("Path to .glsl file…", ctx.import_path)
                        .on_input(Message::ImportPathChanged)
                        .size(11)
                        .width(Fill),
                    btn_secondary("Browse", Message::BrowseImportClicked),
                ]
                .spacing(4),
                row![
                    text_input("Name", ctx.import_name)
                        .on_input(Message::ImportNameChanged)
                        .size(11),
                    text_input("Author", ctx.import_author)
                        .on_input(Message::ImportAuthorChanged)
                        .size(11),
                ]
                .spacing(4),
                row![
                    btn_primary("Import", Message::ImportClicked),
                    text(ctx.import_result).size(10),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(6),
        );

        let dl_btn = if ctx.downloading {
            btn_secondary("Downloading…", Message::DownloadShadertoyClicked)
        } else {
            btn_primary("Download", Message::DownloadShadertoyClicked)
        };

        let download_card = card(
            "Download from Shadertoy",
            column![
                text("Paste a Shadertoy URL or shader ID.")
                    .size(11),
                row![
                    text_input(
                        "URL or ID…",
                        ctx.shadertoy_url,
                    )
                    .on_input(Message::ShadertoyUrlChanged)
                    .size(11)
                    .width(Fill),
                    dl_btn,
                ]
                .spacing(4),
                text(ctx.download_result).size(10),
            ]
            .spacing(6),
        );

        scrollable(
            column![local_card, download_card]
                .spacing(8)
                .padding(8)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}
