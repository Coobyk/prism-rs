use gpui_kit::base::{Disableable, Selectable};
use gpui_kit::component::button::{Button as KitButton, ButtonVariant, ButtonVariants};
use gpui_kit::component::theme::{Theme, ThemeColor, ThemeMode};
use gpui_kit::*;
use prism_core::themes::{ApplicationTheme, Color, ThemeCatalog};

gpui_kit::assets::icon_assets!(
    ExtraAssets,
    [
        Image,
        Pencil,
        Layers,
        Puzzle,
        Database,
        Server,
        Upload,
        Link,
        Notebook,
        Tag,
        PencilLine,
        SlidersHorizontal,
        Package,
    ]
);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(asset) = ExtraAssets.load(path)? {
            return Ok(Some(asset));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraAssets.list(path)?);
        Ok(paths)
    }
}

#[derive(Clone, Copy)]
pub enum Icon {
    New,
    Import,
    Packages,
    Folder,
    Settings,
    Help,
    Update,
    Accounts,
    Launch,
    Kill,
    Edit,
    ChangeIcon,
    Copy,
    Delete,
    Export,
    Shortcut,
    Rename,
    Group,
    Screenshots,
    Worlds,
    Log,
    Version,
    Mods,
    BrowseMods,
    ResourcePacks,
    ShaderPacks,
    DataPacks,
    Servers,
    Notes,
    InstanceSettings,
}

impl Icon {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::New => ("new", "plus"),
            Self::Import => ("import", "inbox"),
            Self::Packages => ("packages", "package"),
            Self::Folder => ("viewfolder", "folder-open"),
            Self::Settings => ("settings", "settings"),
            Self::Help => ("help", "info"),
            Self::Update => ("checkupdate", "refresh-cw"),
            Self::Accounts => ("accounts", "circle-user"),
            Self::Launch => ("launch", "play"),
            Self::Kill => ("status-bad", "ban"),
            Self::Edit => ("settings", "pencil"),
            Self::ChangeIcon => ("settings", "image"),
            Self::Copy => ("copy", "copy"),
            Self::Delete => ("delete", "delete"),
            Self::Export => ("export", "upload"),
            Self::Shortcut => ("shortcut", "link"),
            Self::Rename => ("rename", "pencil-line"),
            Self::Group => ("tag", "tag"),
            Self::Screenshots => ("screenshots", "gallery-vertical-end"),
            Self::Worlds => ("worlds", "map"),
            Self::Log => ("log", "square-terminal"),
            Self::Version => ("minecraft", "layers"),
            Self::Mods => ("loadermods", "puzzle"),
            Self::BrowseMods => ("modrinth", "search"),
            Self::ResourcePacks => ("resourcepacks", "palette"),
            Self::ShaderPacks => ("shaderpacks", "sun"),
            Self::DataPacks => ("datapacks", "database"),
            Self::Servers => ("server", "server"),
            Self::Notes => ("notes", "notebook"),
            Self::InstanceSettings => ("instance-settings", "sliders-horizontal"),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub window: Hsla,
    pub panel: Hsla,
    pub base: Hsla,
    pub alternate: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub strong_border: Hsla,
    pub text: Hsla,
    pub base_text: Hsla,
    pub panel_text: Hsla,
    pub secondary_text: Hsla,
    pub muted_text: Hsla,
    pub disabled_ink: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub highlighted_text: Hsla,
    pub link: Hsla,
    pub error: Hsla,
    pub success: Hsla,
    pub success_bg: Hsla,
    pub warning: Hsla,
    pub warning_bg: Hsla,
    pub tooltip: Hsla,
    pub tooltip_text: Hsla,
    pub transparent: Hsla,
}

impl Global for Palette {}

fn color(c: Color) -> Hsla {
    rgba((c.0 << 8) | (c.0 >> 24)).into()
}

impl Palette {
    fn disabled_foreground(self) -> Hsla {
        self.disabled_ink.opacity(0.5)
    }

    pub fn get(cx: &App) -> Self {
        *cx.global::<Self>()
    }

    pub fn from_theme(theme: &ApplicationTheme) -> Self {
        let light = theme.color("Window").is_light();
        let c = |name| color(theme.color(name));
        let variant = |dark, bright| -> Hsla { rgb(if light { bright } else { dark }).into() };
        let mix = |a: Hsla, b: Hsla, amount| -> Hsla {
            let a: Rgba = a.into();
            let b: Rgba = b.into();
            Rgba {
                r: a.r + (b.r - a.r) * amount,
                g: a.g + (b.g - a.g) * amount,
                b: a.b + (b.b - a.b) * amount,
                a: a.a,
            }
            .into()
        };
        Self {
            window: c("Window"),
            panel: c("Button"),
            base: c("Base"),
            alternate: c("AlternateBase"),
            hover: mix(
                c("Button"),
                c("WindowText"),
                if light { 0.06 } else { 0.055 },
            ),
            border: if light {
                mix(c("Window"), c("WindowText"), 0.16)
            } else {
                mix(c("Window"), rgb(0x000000).into(), 0.2)
            },
            strong_border: mix(c("Button"), c("WindowText"), 0.24),
            text: c("WindowText"),
            base_text: c("Text"),
            panel_text: c("ButtonText"),
            secondary_text: mix(c("WindowText"), c("Window"), 0.27),
            muted_text: mix(c("WindowText"), c("Window"), 0.45),
            disabled_ink: rgb(if theme.color("Button").is_light() {
                0x000000
            } else {
                0xffffff
            })
            .into(),
            accent: c("Highlight"),
            selection: if light {
                mix(c("Window"), c("Highlight"), 0.16)
            } else {
                c("Highlight")
            },
            highlighted_text: c("HighlightedText"),
            link: c("Link"),
            error: c("BrightText"),
            success: variant(0x9fdcb8, 0x216b3c),
            success_bg: variant(0x1e4d2b, 0xe1f2e6),
            warning: variant(0xd8b17c, 0x83540a),
            warning_bg: variant(0x666027, 0xfff1c2),
            tooltip: c("ToolTipBase"),
            tooltip_text: c("ToolTipText"),
            transparent: rgba(0x00000000).into(),
        }
    }
}

struct Selection {
    id: String,
    icon_id: String,
    catalog: ThemeCatalog,
}

impl Global for Selection {}

pub fn init(cx: &mut App) {
    let paths = prism_core::Paths::new();
    let settings = prism_core::settings::Settings::load(&paths.global_config());
    let catalog = ThemeCatalog::load(&paths);
    for warning in &catalog.warnings {
        tracing::warn!("{warning}");
    }
    cx.set_global(Selection {
        id: settings.get_or("ApplicationTheme", "system"),
        icon_id: settings.get_or("IconTheme", "default"),
        catalog,
    });
    apply(cx.window_appearance(), cx);
}

/// Warnings are retained for the Settings theme picker to display.
pub fn catalog(cx: &App) -> &ThemeCatalog {
    &cx.global::<Selection>().catalog
}

/// Applies a selection immediately; the Settings caller owns persistence.
pub fn select(id: &str, appearance: WindowAppearance, cx: &mut App) {
    cx.global_mut::<Selection>().id = id.into();
    apply(appearance, cx);
}

pub fn selected(cx: &App) -> (&str, &str) {
    let selection = cx.global::<Selection>();
    (&selection.id, &selection.icon_id)
}

pub fn select_icons(id: &str, cx: &mut App) {
    cx.global_mut::<Selection>().icon_id = id.into();
    cx.refresh_windows();
}

pub fn reload(
    paths: &prism_core::Paths,
    appearance: WindowAppearance,
    cx: &mut App,
) -> Result<String, String> {
    let catalog = ThemeCatalog::load(paths);
    let (id, icon_id) = selected(cx);
    let id = if catalog.applications.iter().any(|t| t.id == id) {
        id
    } else {
        "system"
    }
    .to_owned();
    let icon_id = if icon_id == "default" || catalog.icons.iter().any(|t| t.id == icon_id) {
        icon_id
    } else {
        "default"
    }
    .to_owned();
    let changed = selected(cx) != (id.as_str(), icon_id.as_str());
    persist(paths, "ApplicationTheme", &id)?;
    persist(paths, "IconTheme", &icon_id)?;
    cx.set_global(Selection {
        id,
        icon_id,
        catalog,
    });
    apply(appearance, cx);
    Ok(if changed {
        "A selected theme was removed. Reverted to System / Default."
    } else {
        "Themes reloaded."
    }
    .into())
}

pub fn persist(paths: &prism_core::Paths, key: &str, id: &str) -> Result<(), String> {
    let path = paths.global_config();
    let mut settings = prism_core::settings::Settings::load(&path);
    settings.set(key, id);
    settings
        .save(&path)
        .map_err(|error| format!("Could not save appearance: {error}"))
}

pub fn custom_icon(catalog: &ThemeCatalog, id: &str, name: &str) -> Option<std::path::PathBuf> {
    if id == "default" {
        return None;
    }
    catalog
        .icons
        .iter()
        .find(|theme| theme.id == id)
        .and_then(|theme| prism_core::themes::icon_path(&theme.directory, name))
}

pub fn icon(icon: Icon, cx: &App) -> AnyElement {
    icon_with_tint(icon, Palette::get(cx).text, true, cx)
}

fn icon_with_tint(icon: Icon, tint: Hsla, custom: bool, cx: &App) -> AnyElement {
    let (name, path) = icon.names();
    let selection = cx.global::<Selection>();
    if let Some(path) = custom
        .then(|| custom_icon(&selection.catalog, &selection.icon_id, name))
        .flatten()
    {
        return img(path)
            .size(px(16.))
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }
    svg()
        .path(format!("icons/{path}.svg"))
        .size(px(16.))
        .text_color(tint)
        .into_any_element()
}

#[derive(IntoElement)]
struct ButtonIcon(Icon);

impl RenderOnce for ButtonIcon {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tint = window.text_style().color;
        let disabled = tint == Palette::get(cx).disabled_foreground();
        icon_with_tint(self.0, tint, !disabled, cx)
    }
}

#[derive(IntoElement)]
pub struct Button {
    inner: KitButton,
    disabled: bool,
}

impl Button {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            inner: KitButton::new(id),
            disabled: false,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.label(label);
        self
    }

    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.inner = self.inner.accessibility_label(label);
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.inner = self.inner.on_click(handler);
        self
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        self.inner.style()
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.inner.extend(elements);
    }
}

impl Disableable for Button {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.inner = self.inner.disabled(disabled);
        self
    }
}

impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self {
        self.inner = self.inner.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.inner.is_selected()
    }
}

impl ButtonVariants for Button {
    fn with_variant(mut self, variant: ButtonVariant) -> Self {
        self.inner = self.inner.with_variant(variant);
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.disabled {
            self.inner
                .text_color(Palette::get(cx).disabled_foreground())
        } else {
            self.inner
        }
    }
}

pub fn button(
    id: impl Into<ElementId>,
    name: Icon,
    label: impl Into<SharedString>,
    _cx: &App,
) -> Button {
    let label = label.into();
    Button::new(id).accessibility_label(label.clone()).child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().size(px(16.)).flex_shrink_0().child(ButtonIcon(name)))
            .child(label),
    )
}

pub fn observe(window: &mut Window, cx: &mut App) {
    apply(window.appearance(), cx);
    window
        .observe_window_appearance(|window, cx| {
            if cx.global::<Selection>().id == "system" {
                apply(window.appearance(), cx);
            }
        })
        .detach();
}

fn apply(appearance: WindowAppearance, cx: &mut App) {
    let selection = cx.global::<Selection>();
    let light = matches!(
        appearance,
        WindowAppearance::Light | WindowAppearance::VibrantLight
    );
    let resolved = selection.catalog.resolve(&selection.id, light);
    let palette = Palette::from_theme(&resolved);
    Theme::change(
        if resolved.color("Window").is_light() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        None,
        cx,
    );
    cx.set_global(palette);
    Theme::update(cx, |theme| sync_colors(&mut theme.colors, palette));
    cx.refresh_windows();
}

fn sync_colors(c: &mut ThemeColor, palette: Palette) {
    c.background = palette.window;
    c.foreground = palette.base_text;
    c.border = palette.border;
    c.input = palette.strong_border;
    c.accent = palette.selection;
    c.accent_foreground = palette.text;
    c.primary = palette.accent;
    c.primary_foreground = palette.highlighted_text;
    c.primary_hover = palette.accent;
    c.primary_active = palette.accent;
    c.button_primary = palette.accent;
    c.button_primary_foreground = palette.highlighted_text;
    c.button_primary_hover = palette.accent;
    c.button_primary_active = palette.accent;
    c.button = palette.panel;
    c.button_foreground = palette.panel_text;
    c.button_hover = palette.hover;
    c.button_active = palette.selection;
    c.secondary = palette.panel;
    c.secondary_foreground = palette.text;
    c.secondary_hover = palette.hover;
    c.secondary_active = palette.selection;
    c.muted = palette.alternate;
    c.muted_foreground = palette.muted_text;
    c.popover = palette.tooltip;
    c.popover_foreground = palette.tooltip_text;
    c.list = palette.base;
    c.list_even = palette.alternate;
    c.list_head = palette.panel;
    c.list_hover = palette.hover;
    c.list_active = palette.selection;
    c.list_active_border = palette.accent;
    c.table = palette.base;
    c.table_even = palette.alternate;
    c.table_head = palette.panel;
    c.table_head_foreground = palette.text;
    c.table_hover = palette.hover;
    c.table_active = palette.selection;
    c.table_active_border = palette.accent;
    c.table_row_border = palette.border;
    c.tab = palette.panel;
    c.tab_bar = palette.panel;
    c.tab_foreground = palette.secondary_text;
    c.tab_active = palette.window;
    c.tab_active_foreground = palette.text;
    c.sidebar = palette.panel;
    c.sidebar_foreground = palette.text;
    c.sidebar_border = palette.border;
    c.sidebar_accent = palette.accent;
    c.sidebar_accent_foreground = palette.highlighted_text;
    c.group_box = palette.panel;
    c.group_box_foreground = palette.text;
    c.title_bar = palette.panel;
    c.title_bar_border = palette.border;
    c.window_border = palette.border;
    c.link = palette.link;
    c.link_hover = palette.link;
    c.link_active = palette.link;
    c.selection = palette.accent;
    c.caret = palette.base_text;
    c.ring = palette.accent;
    c.danger = palette.error;
    c.success = palette.success;
    c.warning = palette.warning;
}

#[cfg(test)]
mod tests {
    use super::{color, Palette};
    use gpui_kit::base::{Disableable, Selectable};
    use gpui_kit::Rgba;
    use prism_core::themes::{ApplicationTheme, Color};

    #[test]
    fn action_icons_are_embedded_and_distinct() {
        use super::Icon::*;
        use gpui_kit::AssetSource;

        let icons = [
            New,
            Import,
            Packages,
            Folder,
            Settings,
            Help,
            Update,
            Accounts,
            Launch,
            Kill,
            Edit,
            ChangeIcon,
            Copy,
            Delete,
            Export,
            Shortcut,
            Rename,
            Group,
            Screenshots,
            Worlds,
            Log,
            Version,
            Mods,
            BrowseMods,
            ResourcePacks,
            ShaderPacks,
            DataPacks,
            Servers,
            Notes,
            InstanceSettings,
        ];
        let mut paths = std::collections::HashSet::new();
        for icon in icons {
            let (key, name) = icon.names();
            assert!(!key.is_empty());
            let path = format!("icons/{name}.svg");
            assert!(
                paths.insert(path.clone()),
                "duplicate built-in icon: {path}"
            );
            assert!(
                !super::Assets.load(&path).unwrap().unwrap().is_empty(),
                "{path}"
            );
        }
        assert!(super::ExtraAssets
            .load("icons/missing.svg")
            .unwrap()
            .is_none());
        assert!(super::Assets.load("icons/missing.svg").is_err());
        assert!(super::ExtraAssets.load("").unwrap().is_none());
    }

    #[test]
    fn icon_fallback_and_fresh_settings_persistence() {
        let root = std::env::temp_dir().join(format!(
            "prism-themes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = prism_core::Paths { root: root.clone() };
        paths.ensure().unwrap();
        let icon_dir = paths.icon_themes_dir().join("colored");
        std::fs::create_dir_all(&icon_dir).unwrap();
        std::fs::write(icon_dir.join("new.svg"), "<svg/>").unwrap();
        let catalog = prism_core::themes::ThemeCatalog::load(&paths);
        assert_eq!(
            super::custom_icon(&catalog, "colored", "new"),
            Some(icon_dir.join("new.svg"))
        );
        assert!(super::custom_icon(&catalog, "colored", "settings").is_none());
        assert!(super::custom_icon(&catalog, "default", "new").is_none());
        assert!(super::custom_icon(&catalog, "missing", "new").is_none());
        let mut settings = prism_core::settings::Settings::default();
        settings.set("MinMemAlloc", "1024");
        settings.save(&paths.global_config()).unwrap();
        super::persist(&paths, "ApplicationTheme", "bright").unwrap();
        super::persist(&paths, "IconTheme", "colored").unwrap();
        let mut general = prism_core::settings::Settings::load(&paths.global_config());
        assert_eq!(general.get("MinMemAlloc"), Some("1024"));
        general.set("MaxMemAlloc", "8192");
        general.save(&paths.global_config()).unwrap();
        let result = prism_core::settings::Settings::load(&paths.global_config());
        assert_eq!(result.get("ApplicationTheme"), Some("bright"));
        assert_eq!(result.get("IconTheme"), Some("colored"));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::write(&root, "not a directory").unwrap();
        assert!(super::persist(&paths, "ApplicationTheme", "dark").is_err());
        std::fs::remove_file(root).unwrap();
    }

    #[test]
    fn qt_alpha_and_semantic_roles() {
        let rgba: Rgba = color(Color(0x80123456)).into();
        assert!((rgba.a - 128. / 255.).abs() < 0.001);
        assert!((rgba.r - 18. / 255.).abs() < 0.001);
        let theme = ApplicationTheme::parse("custom", r##"{"colors":{"Window":"#ffffff","Text":"#112233","ButtonText":"#445566","Highlight":"#123456"}}"##).unwrap();
        let palette = Palette::from_theme(&theme);
        assert_eq!(palette.base_text, color(Color(0xff112233)));
        assert_eq!(palette.panel_text, color(Color(0xff445566)));
        assert_eq!(palette.accent, color(Color(0xff123456)));
    }

    #[test]
    fn placeholder_and_disabled_button_roles() {
        let custom = ApplicationTheme::parse(
            "custom",
            r##"{"colors":{"Window":"#1e1e2e","WindowText":"#cdd6f4","Base":"#181825","AlternateBase":"#242436","Button":"#313244","ButtonText":"#cdd6f4"}}"##,
        )
        .unwrap();
        let luminance = |c: Rgba| {
            let channel = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
        };
        for theme in [
            ApplicationTheme::builtin(false),
            ApplicationTheme::builtin(true),
            ApplicationTheme::parse("empty", "{}").unwrap(),
            custom,
        ] {
            let palette = Palette::from_theme(&theme);
            let mut colors = super::ThemeColor::default();
            super::sync_colors(&mut colors, palette);
            assert_eq!(colors.muted_foreground, palette.muted_text);
            assert_ne!(colors.muted_foreground, palette.disabled_ink);
            assert_eq!(palette.disabled_foreground().a, 0.5);
            let ink: Rgba = palette.disabled_ink.into();
            for background in [
                palette.window,
                palette.panel,
                palette.base,
                palette.alternate,
            ] {
                let bg: Rgba = background.into();
                let placeholder: Rgba = colors.muted_foreground.into();
                let value: Rgba = colors.foreground.into();
                let (p, v, b) = (luminance(placeholder), luminance(value), luminance(bg));
                let placeholder_contrast = (p.max(b) + 0.05) / (p.min(b) + 0.05);
                let value_contrast = (v.max(b) + 0.05) / (v.min(b) + 0.05);
                assert!(placeholder_contrast >= 3., "{}", theme.id);
                assert!(placeholder_contrast < value_contrast * 0.8, "{}", theme.id);
                let fg = Rgba {
                    r: (ink.r + bg.r) / 2.,
                    g: (ink.g + bg.g) / 2.,
                    b: (ink.b + bg.b) / 2.,
                    a: 1.,
                };
                let (a, b) = (luminance(fg), luminance(bg));
                assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 3., "{}", theme.id);
            }
        }
    }

    #[test]
    fn button_state_can_be_updated() {
        let button = super::Button::new("test").disabled(true).selected(true);
        assert!(button.disabled);
        assert!(button.is_selected());
        let button = button.disabled(false).selected(false);
        assert!(!button.disabled);
        assert!(!button.is_selected());
    }

    #[test]
    fn builtin_contrast_and_empty_custom_fallback() {
        let luminance = |c: gpui_kit::Hsla| {
            let rgba: Rgba = c.into();
            let channel = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
        };
        let contrast = |a, b| {
            let a = luminance(a);
            let b = luminance(b);
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for light in [false, true] {
            let palette = Palette::from_theme(&ApplicationTheme::builtin(light));
            assert!(contrast(palette.text, palette.window) >= 7.);
            assert!(contrast(palette.text, palette.selection) >= 4.5);
            assert!(contrast(palette.error, palette.window) >= 4.5);
            assert!(contrast(palette.success, palette.success_bg) >= 4.5);
        }
        let empty = Palette::from_theme(&ApplicationTheme::parse("empty", "{}").unwrap());
        let dark = Palette::from_theme(&ApplicationTheme::builtin(false));
        assert_eq!(empty.window, dark.window);
        assert_eq!(empty.accent, dark.accent);
    }
}
