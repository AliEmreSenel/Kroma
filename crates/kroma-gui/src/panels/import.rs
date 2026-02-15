//! Import panel — Shadertoy URL/file import.

use iced::widget::{column, row, scrollable, text, text_input};
use iced::{Element, Fill};

use crate::panels::dashboard::{btn_primary, btn_secondary, card};
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct ImportPanel;

impl ImportPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for ImportPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let local_card = card(
            "Import Local File",
            column![
                text("Convert a Shadertoy GLSL file into a .shade package.").size(11),
                row![
                    text_input("Path to .glsl file…", ctx.import_path)
                        .on_input(Message::ImportPathChanged)
                        .size(11)
                        .width(Fill),
                    btn_secondary("Browse", Message::BrowseImportClicked, t),
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
                    btn_primary("Import", Message::ImportClicked, t),
                    text(ctx.import_result).size(10),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(6),
            t,
        );

        let dl_btn = if ctx.downloading {
            btn_secondary("Downloading…", Message::DownloadShadertoyClicked, t)
        } else {
            btn_primary("Download", Message::DownloadShadertoyClicked, t)
        };

        let download_card = card(
            "Download from Shadertoy",
            column![
                text("Paste a Shadertoy URL or shader ID.").size(11),
                row![
                    text_input("URL or ID…", ctx.shadertoy_url,)
                        .on_input(Message::ShadertoyUrlChanged)
                        .size(11)
                        .width(Fill),
                    dl_btn,
                ]
                .spacing(4),
                text(ctx.download_result).size(10),
            ]
            .spacing(6),
            t,
        );

        scrollable(
            column![local_card, download_card]
                .spacing(t.spacing_md)
                .padding(t.spacing_md)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}
