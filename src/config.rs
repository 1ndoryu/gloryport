//! Configuración opcional del usuario: `%APPDATA%\GLORYPORT\config.json`.
//!
//! Solo personaliza lo que la derivación automática no acierta; si el archivo
//! no existe o está malformado se usan los valores por defecto (es cosmética,
//! nunca bloquea el escaneo). Formato:
//!
//! ```json
//! {
//!   "workspace": "area-trabajo",
//!   "nombres": { "3000": "Tasks backend" }
//! }
//! ```
//!
//! - `workspace`: carpeta raíz del área de trabajo en las rutas de los procesos
//!   (por defecto `area-trabajo`). El proyecto de cada puerto se deriva de la
//!   carpeta inmediatamente posterior a esa raíz en el cmdline/ruta del proceso.
//! - `nombres`: alias manuales por puerto; sobrescriben el proyecto derivado.
//! - `ocultar`: proyectos/procesos que no se ofrecen (no aparecen en popup ni en
//!   `list` sin `--incluir-sistema`). Matchea case-insensitive contra el proyecto
//!   derivado, el nombre del ejecutable y el script del cmdline.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// Segmento de ruta que marca la raíz del área de trabajo por defecto.
const WORKSPACE_DEFAULT: &str = "area-trabajo";

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Raíz del workspace (opcional; default `area-trabajo`).
    #[serde(default)]
    pub workspace: Option<String>,
    /// Alias manuales por puerto (string → nombre).
    #[serde(default)]
    pub nombres: HashMap<String, String>,
    /// Proyectos/procesos ocultos: no se ofrecen en popup ni en `list`.
    #[serde(default)]
    pub ocultar: Vec<String>,
}

impl Config {
    /// Carga la config del usuario. Un archivo ausente o malformado devuelve
    /// los valores por defecto: es configuración cosmética y jamás debe
    /// impedir listar o matar puertos.
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Config::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(texto) => serde_json::from_str(&texto).unwrap_or_default(),
            Err(_) => Config::default(),
        }
    }

    /// Segmento que marca la raíz del workspace (comparado case-insensitive).
    pub fn workspace_raiz(&self) -> &str {
        self.workspace
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(WORKSPACE_DEFAULT)
    }

    /// Alias manual para un puerto, si está configurado.
    pub fn alias_para(&self, port: u16) -> Option<&str> {
        self.nombres.get(&port.to_string()).map(String::as_str)
    }

    /// ¿Un candidato (proyecto derivado, proceso o script) está marcado como
    /// oculto en la config? Case-insensitive.
    pub fn esta_oculto(&self, candidato: &str) -> bool {
        self.ocultar
            .iter()
            .any(|oculto| oculto.eq_ignore_ascii_case(candidato))
    }
}

/// `%APPDATA%\GLORYPORT\config.json` (la misma carpeta que usa el auto-inicio).
fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("GLORYPORT").join("config.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_sin_archivo() {
        let cfg = Config::default();
        assert_eq!(cfg.workspace_raiz(), "area-trabajo");
        assert_eq!(cfg.alias_para(3000), None);
    }

    #[test]
    fn parsea_json_valido() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspace": "workspace", "nombres": {"3000": "Tasks backend"}}"#,
        )
        .unwrap();
        assert_eq!(cfg.workspace_raiz(), "workspace");
        assert_eq!(cfg.alias_para(3000), Some("Tasks backend"));
        assert_eq!(cfg.alias_para(3101), None);
    }

    #[test]
    fn ocultar_matchea_case_insensitive() {
        let cfg: Config = serde_json::from_str(r#"{"ocultar": ["gloryapi", "intel"]}"#).unwrap();
        assert!(cfg.esta_oculto("gloryapi"));
        assert!(cfg.esta_oculto("GLORYAPI"));
        assert!(cfg.esta_oculto("Intel"));
        assert!(!cfg.esta_oculto("glory-port"));
        assert!(!Config::default().esta_oculto("gloryapi"));
    }

    #[test]
    fn json_vacio_usa_defaults() {
        let cfg: Config = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(cfg.workspace_raiz(), "area-trabajo");
    }

    #[test]
    fn json_malformado_cae_a_defaults() {
        let cfg: Config = serde_json::from_str("no es json").unwrap_or_default();
        assert_eq!(cfg.workspace_raiz(), "area-trabajo");
    }
}
