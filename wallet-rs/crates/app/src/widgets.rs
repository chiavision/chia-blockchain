//! Small reusable building blocks.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt, AnyElement, App, ClickEvent, Div, ElementId, FontWeight, Rgba, SharedString, Svg,
    Transformation, Window, div, percentage, px, svg,
};

use crate::assets::Icon;
use crate::theme;

gpui::actions!(wallet, [Dismiss]);

pub fn icon(i: Icon) -> Svg {
    svg().path(i.path()).size_4().flex_none().text_color(theme::text_dim())
}

pub fn spinner(id: impl Into<ElementId>) -> impl IntoElement {
    icon(Icon::Spinner).text_color(theme::accent_hi()).with_animation(
        id,
        Animation::new(Duration::from_millis(850)).repeat(),
        |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
    )
}

pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme::surface())
        .border_1()
        .border_color(theme::border())
        .rounded_xl()
}

/// Small upper-case section label.
pub fn eyebrow(text: impl Into<SharedString>) -> Div {
    let t: SharedString = text.into();
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::muted())
        .child(SharedString::from(t.to_uppercase()))
}

pub fn badge(text: impl Into<SharedString>, fg: Rgba, bg: Rgba) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py(px(2.))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

pub fn dot(color: Rgba) -> Div {
    div().size(px(7.)).rounded_full().bg(color).flex_none()
}

pub fn mono(text: impl Into<SharedString>) -> Div {
    div().font_family(theme::MONO).child(text.into())
}

pub fn callout(kind: Callout, title: impl Into<SharedString>, body: impl Into<SharedString>) -> Div {
    let (fg, bg, ic) = match kind {
        Callout::Warning => (theme::warning(), theme::warning_soft(), Icon::Alert),
        Callout::Danger => (theme::danger(), theme::danger_soft(), Icon::Alert),
        Callout::Safe => (theme::accent_hi(), theme::accent_soft(), Icon::Shield),
    };
    div()
        .flex()
        .gap_3()
        .p_3()
        .rounded_lg()
        .bg(bg)
        .border_1()
        .border_color(Rgba { a: 0.35, ..fg })
        .child(icon(ic).text_color(fg).mt(px(1.)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(title.into()),
                )
                .child(div().text_sm().text_color(theme::text_dim()).child(body.into())),
        )
}

#[derive(Clone, Copy)]
pub enum Callout {
    Warning,
    Danger,
    Safe,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    variant: Variant,
    icon: Option<Icon>,
    disabled: bool,
    full: bool,
    large: bool,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            variant: Variant::Secondary,
            icon: None,
            disabled: false,
            full: false,
            large: false,
            on_click: None,
        }
    }

    pub fn variant(mut self, v: Variant) -> Self {
        self.variant = v;
        self
    }

    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }

    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }

    pub fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }

    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }

    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    pub fn full_width(mut self) -> Self {
        self.full = true;
        self
    }

    pub fn large(mut self) -> Self {
        self.large = true;
        self
    }

    pub fn on_click(mut self, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(f));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (bg, fg, hover, border) = match self.variant {
            Variant::Primary => (theme::accent(), theme::rgb_white(), theme::accent_hi(), theme::accent()),
            Variant::Secondary => (
                theme::surface_hi(),
                theme::text(),
                theme::surface_hover(),
                theme::border_hi(),
            ),
            Variant::Ghost => (
                theme::transparent(),
                theme::text_dim(),
                theme::surface_hover(),
                theme::transparent(),
            ),
            Variant::Danger => (
                theme::danger_soft(),
                theme::danger(),
                Rgba {
                    a: 0.25,
                    ..theme::danger()
                },
                Rgba {
                    a: 0.4,
                    ..theme::danger()
                },
            ),
        };
        let disabled = self.disabled;
        let mut el = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap_2()
            .px(if self.large { px(20.) } else { px(14.) })
            .h(if self.large { px(44.) } else { px(34.) })
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .when(self.full, |d| d.w_full())
            .when_some(self.icon, |d, i| d.child(icon(i).text_color(fg)))
            .child(self.label);
        if disabled {
            el = el.opacity(0.4).cursor_not_allowed();
        } else {
            el = el.cursor_pointer().hover(move |s| s.bg(hover));
            if let Some(handler) = self.on_click {
                el = el.on_click(handler);
            }
        }
        el
    }
}

/// A square icon-only button.
pub fn icon_button(id: impl Into<ElementId>, i: Icon) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded_lg()
        .cursor_pointer()
        .hover(|s| s.bg(theme::surface_hover()))
        .child(icon(i))
}

/// A modal dialog over a dimmed backdrop. Mouse events don't reach the page below.
pub fn modal(id: impl Into<ElementId>, width: f32, content: impl IntoElement) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::scrim())
        .occlude()
        .child(
            card()
                .w(px(width))
                .max_w_full()
                .p_6()
                .gap_4()
                .bg(theme::surface_hi())
                .border_color(theme::border_hi())
                .shadow_lg()
                .child(content),
        )
        .with_animation("modal-in", Animation::new(Duration::from_millis(140)), |el, delta| {
            el.opacity(delta)
        })
        .into_any_element()
}
