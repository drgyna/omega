//! Motor aritmético verificable.
//!
//! Todas las cantidades se manejan como enteros de escala fija: una suma de
//! importes nunca acumula el error de un binario flotante y dos ejecuciones
//! sobre el mismo acervo devuelven exactamente el mismo dígito. El módulo no
//! conoce ningún campo ni giro de negocio: recibe valores ya extraídos con su
//! evidencia y sólo decide cómo combinarlos.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::Evidence;

/// Escala fija: cuatro dígitos a la derecha del punto cubren tanto importes de
/// dos decimales como los cocientes de un promedio sin redondear de más.
const SCALE: i128 = 10_000;

/// Cantidad con escala fija. La representación interna es el valor multiplicado
/// por 10^SCALE_DIGITS, de modo que sumar y restar son operaciones exactas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Decimal(i128);

impl Decimal {
    pub const ZERO: Self = Self(0);

    pub fn from_units(units: i128) -> Self {
        Self(units.saturating_mul(SCALE))
    }

    /// Convierte un valor leído del índice. Devuelve `None` para entradas que
    /// no representan una cantidad utilizable en lugar de sustituirlas por
    /// cero, que falsearía un promedio o un mínimo.
    pub fn from_f64(value: f64) -> Option<Self> {
        if !value.is_finite() {
            return None;
        }
        let scaled = (value * SCALE as f64).round();
        (scaled.abs() < 9.0e30).then(|| Self(scaled as i128))
    }

    /// Convierte el literal extraído sin pasar por `f64`. Los operandos de
    /// producción conservan el texto original, por lo que una suma de 0.10 y
    /// 0.20 se representa exactamente como 0.30.
    pub fn from_text(value: &str) -> Option<Self> {
        let mut value = value.trim();
        let mut negative = false;
        if let Some(rest) = value.strip_prefix('-') {
            negative = true;
            value = rest.trim_start();
        }
        if let Some(symbol) = value.chars().next().filter(|symbol| {
            matches!(symbol, '$' | '€' | '£' | '¥' | '₹' | '₩')
        }) {
            value = value.strip_prefix(symbol).expect("checked symbol").trim_start();
        }
        if let Some(rest) = value.strip_prefix('-') {
            if negative {
                return None;
            }
            negative = true;
            value = rest.trim_start();
        }
        value = value.strip_suffix('%').unwrap_or(value).trim();
        value = value.trim_end_matches(|character: char| {
            character.is_ascii_alphabetic() || character.is_whitespace()
        });
        let value = value.replace(',', "");
        let (whole, fraction) = value.split_once('.').unwrap_or((&value, ""));
        if whole.is_empty()
            || !whole.chars().all(|character| character.is_ascii_digit())
            || !fraction.chars().all(|character| character.is_ascii_digit())
            || fraction.len() > 4
        {
            return None;
        }
        let whole = whole.parse::<i128>().ok()?;
        let fraction = format!("{fraction:0<4}").parse::<i128>().ok()?;
        let raw = whole.checked_mul(SCALE)?.checked_add(fraction)?;
        Some(Self(if negative { -raw } else { raw }))
    }

    /// Representación interna, para guardar una cantidad en el contexto sin
    /// perder precisión y reconstruirla después.
    pub fn raw(self) -> i128 {
        self.0
    }

    pub fn from_raw(units: i128) -> Self {
        Self(units)
    }

    pub fn add(self, other: Self) -> Self {
        Self(self.0 + other.0)
    }

    pub fn sub(self, other: Self) -> Self {
        Self(self.0 - other.0)
    }

    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Promedio exacto hasta la escala del módulo. `None` sin valores: un
    /// promedio de cero elementos no existe y no debe inventarse.
    pub fn divide_by_count(self, count: usize) -> Option<Self> {
        (count > 0).then(|| Self(round_divide(self.0, count as i128)))
    }

    /// Variación porcentual entre dos cantidades. `None` cuando la base es
    /// cero: el cambio no está definido y el motor debe explicarlo en vez de
    /// devolver una cifra.
    pub fn percent_change(from: Self, to: Self) -> Option<Self> {
        (from.0 != 0).then(|| Self(round_divide((to.0 - from.0) * 100 * SCALE, from.0)))
    }

    /// Representación con separador de miles. Las cantidades enteras no
    /// muestran decimales; las fraccionarias conservan al menos dos.
    pub fn render(self) -> String {
        let negative = self.0 < 0;
        let magnitude = self.0.unsigned_abs();
        let integer = magnitude / SCALE as u128;
        let fraction = (magnitude % SCALE as u128) as u32;
        let mut digits = format!("{fraction:04}");
        while digits.len() > 2 && digits.ends_with('0') {
            digits.pop();
        }
        let rendered = if fraction == 0 {
            group_thousands(&integer.to_string())
        } else {
            format!("{}.{}", group_thousands(&integer.to_string()), digits)
        };
        if negative {
            format!("-{rendered}")
        } else {
            rendered
        }
    }

    /// Formato monetario: nunca menos de dos decimales.
    pub fn render_money(self) -> String {
        let rendered = self.render();
        if rendered.contains('.') {
            rendered
        } else {
            format!("{rendered}.00")
        }
    }

    /// Porcentajes con signo explícito: el signo es parte del hecho reportado.
    pub fn render_signed(self) -> String {
        if self.0 > 0 {
            format!("+{}", self.render())
        } else {
            self.render()
        }
    }

    /// Producto exacto en la escala fija: el resultado intermedio se calcula
    /// en la escala al cuadrado y se reduce una sola vez, para no perder
    /// precisión con un redondeo por cada paso.
    pub fn multiply(self, other: Self) -> Self {
        Self(round_divide(self.0.saturating_mul(other.0), SCALE))
    }

    /// Cociente exacto hasta la escala del módulo. Dividir entre cero es
    /// responsabilidad de quien llama: aquí sólo se protege de un pánico,
    /// nunca se presenta como un resultado real (`round_divide` ya devuelve
    /// 0 en ese caso, pero ese 0 no debe llegar a una respuesta verificada).
    pub fn divide(self, other: Self) -> Self {
        Self(round_divide(self.0.saturating_mul(SCALE), other.0))
    }
}

fn round_divide(numerator: i128, denominator: i128) -> i128 {
    if denominator == 0 {
        return 0;
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder == 0 {
        return quotient;
    }
    let rounds_up = remainder.saturating_mul(2).abs() >= denominator.abs();
    let sign = if (numerator < 0) != (denominator < 0) {
        -1
    } else {
        1
    };
    if rounds_up { quotient + sign } else { quotient }
}

fn group_thousands(digits: &str) -> String {
    let mut grouped = String::new();
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

/// Operación aritmética pedida. Es un dato del plan, no una cadena libre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Sum,
    Average,
    Minimum,
    Maximum,
    Count,
}

impl Operation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sum => "suma",
            Self::Average => "promedio",
            Self::Minimum => "mínimo",
            Self::Maximum => "máximo",
            Self::Count => "conteo",
        }
    }

    pub fn needs_numbers(self) -> bool {
        !matches!(self, Self::Count)
    }

    /// Recupera la operación desde la etiqueta guardada en el contexto, para
    /// que «compáralo con el mes anterior» repita la misma operación del turno
    /// previo en vez de suponer una.
    pub fn from_label(label: &str) -> Option<Self> {
        [
            Self::Sum,
            Self::Average,
            Self::Minimum,
            Self::Maximum,
            Self::Count,
        ]
        .into_iter()
        .find(|operation| operation.label() == label)
    }
}

/// Un valor del acervo con su procedencia. La aritmética nunca ve texto suelto:
/// cada operando llega acompañado de la evidencia que lo respalda.
#[derive(Clone, Debug)]
pub struct Operand {
    pub document_id: i64,
    pub numeric: Option<f64>,
    pub currency: Option<String>,
    pub group: Option<String>,
    pub evidence: Evidence,
}

/// Resultado de una operación dentro de una sola moneda y un solo grupo.
#[derive(Clone, Debug)]
pub struct Bucket {
    pub group: Option<String>,
    pub currency: Option<String>,
    pub value: Decimal,
    pub value_count: usize,
    pub document_ids: BTreeSet<i64>,
    pub evidence: Vec<Evidence>,
    /// Se conserva aunque la evidencia concreta quede fuera de la muestra
    /// visible. La verificación nunca depende de que el operando débil haya
    /// cabido entre las primeras citas.
    pub has_unreliable_evidence: bool,
}

const MAX_BUCKET_EVIDENCE: usize = 50;

/// Aplica la operación separando por moneda y por grupo.
///
/// La separación por moneda no es configurable: dos importes de monedas
/// distintas jamás caen en el mismo acumulador, aunque la pregunta no haya
/// mencionado ninguna moneda.
pub fn compute(operation: Operation, operands: &[Operand]) -> Vec<Bucket> {
    let mut buckets: BTreeMap<(Option<String>, Option<String>), Bucket> = BTreeMap::new();
    for operand in operands {
        let exact_amount = operand
            .evidence
            .value
            .as_deref()
            .and_then(Decimal::from_text);
        // La ruta de producción siempre adjunta el literal extraído y por eso
        // no pasa por `f64`. El respaldo sólo conserva las fixtures internas
        // antiguas, que construyen operandos sintéticos sin texto fuente.
        let amount = match exact_amount.or_else(|| {
            operand
                .evidence
                .value
                .is_none()
                .then(|| operand.numeric.and_then(Decimal::from_f64))
                .flatten()
        }) {
            Some(value) => Some(value),
            None if operation.needs_numbers() => continue,
            None => None,
        };
        // Un conteo no tiene dimensión monetaria: contar cuántos valores hay no
        // es una cantidad de dinero y no debe presentarse como tal.
        let currency = (operation != Operation::Count)
            .then(|| operand.currency.clone())
            .flatten();
        let key = (operand.group.clone(), currency.clone());
        let bucket = buckets.entry(key).or_insert_with(|| Bucket {
            group: operand.group.clone(),
            currency,
            value: Decimal::ZERO,
            value_count: 0,
            document_ids: BTreeSet::new(),
            evidence: Vec::new(),
            has_unreliable_evidence: false,
        });
        match (operation, amount) {
            (Operation::Sum | Operation::Average, Some(value)) => {
                bucket.value = bucket.value.add(value);
            }
            (Operation::Minimum, Some(value)) => {
                if bucket.value_count == 0 || value < bucket.value {
                    bucket.value = value;
                }
            }
            (Operation::Maximum, Some(value)) => {
                if bucket.value_count == 0 || value > bucket.value {
                    bucket.value = value;
                }
            }
            (Operation::Count, _) => {
                bucket.value = bucket.value.add(Decimal::from_units(1));
            }
            _ => {}
        }
        bucket.value_count += 1;
        bucket.document_ids.insert(operand.document_id);
        bucket.has_unreliable_evidence |= !operand.evidence.reliable;
        if bucket.evidence.len() < MAX_BUCKET_EVIDENCE {
            bucket.evidence.push(operand.evidence.clone());
        }
    }
    let mut rows = buckets.into_values().collect::<Vec<_>>();
    if operation == Operation::Average {
        for row in &mut rows {
            row.value = row
                .value
                .divide_by_count(row.value_count)
                .unwrap_or(Decimal::ZERO);
        }
    }
    rows.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| left.currency.cmp(&right.currency))
    });
    rows
}

/// ¿Puede declararse enteramente verificado un resultado que combina varios
/// campos en una sola tabla (por ejemplo, «ponme todos» sobre una
/// aclaración)?
///
/// Sólo si CADA campo pedido tiene al menos un valor en el alcance y esos
/// valores no mezclan monedas entre sí. Un campo sin datos se muestra como
/// «Sin datos» en la tabla —nunca como cero— pero le quita a la respuesta
/// completa el derecho a declararse verificada, igual que un campo cuyos
/// propios valores están repartidos en monedas distintas: ninguna de las dos
/// situaciones puede resumirse con una sola cifra confiable.
pub fn multi_field_is_fully_verified(results: &[(String, Vec<Bucket>)]) -> bool {
    results.iter().all(|(_, buckets)| {
        !buckets.is_empty()
            && buckets
                .iter()
                .map(|bucket| &bucket.currency)
                .collect::<BTreeSet<_>>()
                .len()
                == 1
    })
}

/// Operación entre **dos campos numéricos del mismo documento**
/// («Cantidad × Precio unitario», «Monto A − Monto B», «Monto A ÷ Monto B»).
///
/// Es deliberadamente un tipo distinto de `Operation`: `Operation` reduce
/// muchos valores de un solo campo a una cifra; esto combina dos campos
/// distintos, documento por documento. El total, si se pide, es la suma de
/// esos resultados por documento — nunca el resultado de operar entre los
/// totales globales de cada campo, que sería una cifra distinta de la que se
/// pidió y que el motor no calcula sin que la pregunta lo diga sin ambigüedad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowOperation {
    Subtract,
    Multiply,
    Divide,
}

impl RowOperation {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Subtract => "menos",
            Self::Multiply => "por",
            Self::Divide => "entre",
        }
    }

    /// Nombre de la operación con el que se encabeza el resultado. Nunca es
    /// «Suma»: una multiplicación fila por fila cuyos productos se acumulan
    /// sigue siendo una multiplicación, y llamarla suma describía mal lo que
    /// el motor hizo.
    pub fn title(self) -> &'static str {
        match self {
            Self::Subtract => "Resta",
            Self::Multiply => "Multiplicación",
            Self::Divide => "División",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Subtract => "−",
            Self::Multiply => "×",
            Self::Divide => "÷",
        }
    }
}

/// Resultado de aplicar la operación en un documento donde ambos campos
/// tenían un valor numérico y unidades compatibles.
#[derive(Clone, Debug)]
pub struct RowOutcome {
    // No lo lee el código de producción (agent.rs trabaja con los conteos
    // agregados), pero las pruebas de este módulo lo usan para comprobar que
    // el documento correcto produjo el resultado correcto.
    #[allow(dead_code)]
    pub document_id: i64,
    pub value: Decimal,
    pub currency: Option<String>,
    pub left_rendered: String,
    pub right_rendered: String,
    pub left_evidence: Evidence,
    pub right_evidence: Evidence,
}

/// Por qué un documento con ambos campos no produjo un resultado. Nunca se
/// convierte en un cero ni en un valor inventado: se cuenta y se explica.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowIssue {
    /// El divisor era exactamente cero.
    DivisionByZero,
    /// Las unidades de los dos campos no pueden combinarse con esta
    /// operación (por ejemplo, restar dos monedas distintas).
    IncompatibleUnits,
    /// El documento tenía los dos campos, pero al menos uno de los dos no
    /// pudo leerse como un número (por ejemplo, «N/A» en un campo que en
    /// otros documentos sí trae una cifra). No es lo mismo que un campo
    /// ausente: el campo está, su valor no sirve.
    InvalidValue,
}

#[derive(Clone, Debug)]
pub struct RowSkip {
    #[allow(dead_code)]
    pub document_id: i64,
    pub issue: RowIssue,
    pub left_evidence: Evidence,
    pub right_evidence: Evidence,
}

/// Clasificación de **todos** los documentos del alcance en categorías
/// mutuamente excluyentes.
///
/// El invariante que sostiene la respuesta es
/// `scope_documents == calculated + excluded()`: ningún documento del alcance
/// puede quedar sin explicación. Antes faltaba precisamente la categoría
/// `neither_field`: un documento sin ninguno de los dos campos no aparece en
/// `left` ni en `right`, así que era invisible para el cálculo y se esfumaba
/// de la cuenta — el alcance decía 600, los calculados 140, y los 460
/// restantes no se mencionaban en ninguna parte.
///
/// Se cuenta por documento, no por operando: un documento con dos valores del
/// mismo campo produce dos resultados, pero sigue siendo un solo documento del
/// alcance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowScopeBreakdown {
    /// Documentos que el filtro de la pregunta dejó en el alcance.
    pub scope_documents: usize,
    /// Documentos que produjeron al menos un resultado.
    pub calculated: usize,
    /// Tenían los dos campos, pero al menos uno no era un número utilizable.
    pub invalid_value: usize,
    /// Tenían los dos campos, con unidades que no pueden combinarse.
    pub incompatible_units: usize,
    /// Tenían los dos campos, pero el divisor era exactamente cero.
    pub division_by_zero: usize,
    /// Tenían exactamente uno de los dos campos.
    pub one_field_only: usize,
    /// No tenían ninguno de los dos campos.
    pub neither_field: usize,
}

impl RowScopeBreakdown {
    /// Todos los documentos del alcance que no produjeron una cifra, por
    /// cualquiera de las razones anteriores.
    pub fn excluded(&self) -> usize {
        self.invalid_value
            + self.incompatible_units
            + self.division_by_zero
            + self.one_field_only
            + self.neither_field
    }

    /// El invariante: cada documento del alcance cae en exactamente una
    /// categoría. `compute_row` lo construye recorriendo el alcance completo,
    /// así que se cumple por construcción; las pruebas lo comprueban para que
    /// no deje de cumplirse si alguien cambia la clasificación.
    pub fn is_exhaustive(&self) -> bool {
        self.calculated + self.excluded() == self.scope_documents
    }
}

/// A qué categoría pertenece un documento que sí tenía los dos campos.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowCategory {
    Calculated,
    Excluded(RowIssue),
}

/// Resultado completo de combinar dos campos documento por documento.
#[derive(Clone, Debug, Default)]
pub struct RowComputation {
    pub outcomes: Vec<RowOutcome>,
    pub skipped: Vec<RowSkip>,
    /// Cómo se reparte el alcance completo entre calculados y excluidos.
    pub breakdown: RowScopeBreakdown,
    /// Igual que en `Bucket`, representa todos los operandos usados, no sólo
    /// la ventana de citas que se devuelve al usuario.
    pub has_unreliable_evidence: bool,
}

/// Decide la unidad resultante de combinar dos campos, o rechaza la
/// combinación si las unidades son incompatibles.
///
/// Semántica, documentada porque no se deduce del tipo:
/// - **Multiplicar**: cantidad (sin moneda) × dinero = dinero; dinero × dinero
///   no tiene una unidad clara y se rechaza.
/// - **Restar**: sólo entre la misma moneda (o ambos sin moneda); monedas
///   distintas, o una cantidad con moneda y otra sin ella, se rechazan.
/// - **Dividir**: dinero ÷ la misma moneda = proporción adimensional; dinero ÷
///   cantidad = dinero por unidad; cantidad ÷ dinero no tiene una unidad clara
///   y se rechaza.
fn combined_currency(
    operation: RowOperation,
    left: Option<&str>,
    right: Option<&str>,
) -> Result<Option<String>, RowIssue> {
    match operation {
        RowOperation::Multiply => match (left, right) {
            (Some(_), Some(_)) => Err(RowIssue::IncompatibleUnits),
            (Some(currency), None) | (None, Some(currency)) => Ok(Some(currency.to_owned())),
            (None, None) => Ok(None),
        },
        RowOperation::Subtract => match (left, right) {
            (Some(a), Some(b)) if a == b => Ok(Some(a.to_owned())),
            (None, None) => Ok(None),
            _ => Err(RowIssue::IncompatibleUnits),
        },
        RowOperation::Divide => match (left, right) {
            (Some(a), Some(b)) if a == b => Ok(None),
            (Some(_), Some(_)) => Err(RowIssue::IncompatibleUnits),
            (Some(currency), None) => Ok(Some(currency.to_owned())),
            (None, Some(_)) => Err(RowIssue::IncompatibleUnits),
            (None, None) => Ok(None),
        },
    }
}

/// Combina dos campos documento por documento. Empareja por `document_id`:
/// un documento que sólo tiene uno de los dos campos no participa y no se le
/// inventa un cero para el que falta.
///
/// `scope_documents` es el alcance completo de la pregunta —no sólo los
/// documentos que traen alguno de los dos campos—, porque es la única forma
/// de contar los que no traen ninguno: esos no aparecen en `left` ni en
/// `right` y, sin el alcance, serían invisibles para el cálculo.
pub fn compute_row(
    operation: RowOperation,
    left: &[Operand],
    right: &[Operand],
    scope_documents: &[i64],
) -> RowComputation {
    let mut right_by_document: BTreeMap<i64, &Operand> = BTreeMap::new();
    for operand in right {
        right_by_document.entry(operand.document_id).or_insert(operand);
    }
    let mut outcomes = Vec::new();
    let mut skipped = Vec::new();
    let mut has_unreliable_evidence = false;
    for candidate in left {
        let Some(other) = right_by_document.get(&candidate.document_id) else {
            continue;
        };
        has_unreliable_evidence |= !candidate.evidence.reliable || !other.evidence.reliable;
        let (Some(left_value), Some(right_value)) = (
            candidate.numeric.and_then(Decimal::from_f64),
            other.numeric.and_then(Decimal::from_f64),
        ) else {
            // Los dos campos están presentes, pero al menos uno no es un
            // número utilizable: se cuenta y se explica, igual que una
            // unidad incompatible o una división entre cero. Descartarlo en
            // silencio dejaría un documento que sí se examinó fuera de las
            // tres cuentas (calculado, incompatible, sin un campo) que la
            // respuesta declara.
            skipped.push(RowSkip {
                document_id: candidate.document_id,
                issue: RowIssue::InvalidValue,
                left_evidence: candidate.evidence.clone(),
                right_evidence: other.evidence.clone(),
            });
            continue;
        };
        match combined_currency(operation, candidate.currency.as_deref(), other.currency.as_deref()) {
            Err(issue) => skipped.push(RowSkip {
                document_id: candidate.document_id,
                issue,
                left_evidence: candidate.evidence.clone(),
                right_evidence: other.evidence.clone(),
            }),
            Ok(currency) => {
                if operation == RowOperation::Divide && right_value == Decimal::ZERO {
                    skipped.push(RowSkip {
                        document_id: candidate.document_id,
                        issue: RowIssue::DivisionByZero,
                        left_evidence: candidate.evidence.clone(),
                        right_evidence: other.evidence.clone(),
                    });
                    continue;
                }
                let value = match operation {
                    RowOperation::Subtract => left_value.sub(right_value),
                    RowOperation::Multiply => left_value.multiply(right_value),
                    RowOperation::Divide => left_value.divide(right_value),
                };
                outcomes.push(RowOutcome {
                    document_id: candidate.document_id,
                    value,
                    currency,
                    left_rendered: render_amount(left_value, candidate.currency.as_deref()),
                    right_rendered: render_amount(right_value, other.currency.as_deref()),
                    left_evidence: candidate.evidence.clone(),
                    right_evidence: other.evidence.clone(),
                });
            }
        }
    }
    let left_documents = left.iter().map(|o| o.document_id).collect::<BTreeSet<_>>();
    let right_documents = right.iter().map(|o| o.document_id).collect::<BTreeSet<_>>();

    // Categoría de cada documento que sí tenía los dos campos. Un documento
    // que produjo al menos un resultado cuenta como calculado aunque otro de
    // sus valores se haya descartado: la cifra publicada sí lo incluye.
    let mut category: BTreeMap<i64, RowCategory> = BTreeMap::new();
    for outcome in &outcomes {
        category.insert(outcome.document_id, RowCategory::Calculated);
    }
    for skip in &skipped {
        category
            .entry(skip.document_id)
            .or_insert(RowCategory::Excluded(skip.issue));
    }

    // Recorre el alcance completo, no las listas de operandos: así cada
    // documento cae en exactamente una categoría y el invariante
    // `alcance == calculados + excluidos` se cumple por construcción.
    let scope = scope_documents.iter().copied().collect::<BTreeSet<_>>();
    let mut breakdown = RowScopeBreakdown {
        scope_documents: scope.len(),
        ..RowScopeBreakdown::default()
    };
    for id in &scope {
        match category.get(id) {
            Some(RowCategory::Calculated) => breakdown.calculated += 1,
            Some(RowCategory::Excluded(RowIssue::DivisionByZero)) => {
                breakdown.division_by_zero += 1
            }
            Some(RowCategory::Excluded(RowIssue::IncompatibleUnits)) => {
                breakdown.incompatible_units += 1
            }
            Some(RowCategory::Excluded(RowIssue::InvalidValue)) => breakdown.invalid_value += 1,
            None => {
                if left_documents.contains(id) || right_documents.contains(id) {
                    breakdown.one_field_only += 1;
                } else {
                    breakdown.neither_field += 1;
                }
            }
        }
    }

    RowComputation {
        outcomes,
        skipped,
        breakdown,
        has_unreliable_evidence,
    }
}

/// Formato de una cantidad con su moneda. Sin moneda no se antepone el símbolo:
/// un número que no es dinero no debe presentarse como si lo fuera.
///
/// El código siempre se muestra tal como está en el acervo: es la única
/// etiqueta fiable. El símbolo es sólo un adorno adicional para las monedas
/// cuyo símbolo es inequívoco (un «$» sirve para USD y para MXN a la vez, así
/// que nunca sustituye al código); una moneda sin símbolo conocido se muestra
/// con su código y sin inventar un signo que no le corresponde.
pub fn render_amount(value: Decimal, currency: Option<&str>) -> String {
    match currency {
        // Una cantidad monetaria derivada se presenta siempre con sus
        // centavos: «$6,000» y «$6,000.00» se leen igual, pero la segunda
        // forma deja claro que el cálculo llegó hasta el último decimal.
        Some(code) => {
            let rendered = value.render_money();
            let symbol = currency_symbol(code).unwrap_or("");
            match rendered.strip_prefix('-') {
                Some(magnitude) => format!("-{symbol}{magnitude} {code}"),
                None => format!("{symbol}{rendered} {code}"),
            }
        }
        None => value.render(),
    }
}

/// Símbolo inequívoco de un código de moneda ISO conocido. `None` para
/// cualquier código que el motor no reconozca: mostrar el código solo es
/// preferible a adivinar un símbolo que podría ser el de otra moneda.
fn currency_symbol(code: &str) -> Option<&'static str> {
    match code.to_ascii_uppercase().as_str() {
        "USD" | "MXN" | "CAD" | "AUD" | "NZD" | "HKD" | "SGD" | "ARS" | "CLP" | "COP" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "JPY" | "CNY" => Some("¥"),
        "INR" => Some("₹"),
        "KRW" => Some("₩"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_sums_do_not_drift_like_binary_floats() {
        // 0.1 + 0.2 en f64 no es 0.3; en escala fija sí lo es.
        let total = Decimal::from_f64(0.1)
            .unwrap()
            .add(Decimal::from_f64(0.2).unwrap());
        assert_eq!(total, Decimal::from_f64(0.3).unwrap());
        assert_eq!(total.render(), "0.30");
    }

    #[test]
    fn a_long_sum_of_cents_stays_exact() {
        let mut total = Decimal::ZERO;
        for _ in 0..1_000 {
            total = total.add(Decimal::from_f64(0.07).unwrap());
        }
        assert_eq!(total.render(), "70");
    }

    #[test]
    fn averages_round_half_away_from_zero_at_the_fixed_scale() {
        let total = Decimal::from_f64(10.0).unwrap();
        assert_eq!(total.divide_by_count(3).unwrap().render(), "3.3333");
        assert_eq!(Decimal::from_f64(1.0).unwrap().divide_by_count(0), None);
    }

    #[test]
    fn percent_change_is_undefined_against_a_zero_base() {
        let from = Decimal::from_f64(200.0).unwrap();
        let to = Decimal::from_f64(250.0).unwrap();
        assert_eq!(
            Decimal::percent_change(from, to).unwrap().render_signed(),
            "+25"
        );
        assert_eq!(
            Decimal::percent_change(to, from).unwrap().render_signed(),
            "-20"
        );
        assert_eq!(Decimal::percent_change(Decimal::ZERO, to), None);
    }

    #[test]
    fn render_amount_shows_the_real_code_instead_of_a_fixed_dollar_sign() {
        let value = Decimal::from_f64(1_200.0).unwrap();
        assert_eq!(render_amount(value, Some("MXN")), "$1,200.00 MXN");
        assert_eq!(render_amount(value, Some("USD")), "$1,200.00 USD");
        // El euro no usa «$»: inventar el signo del dólar sería mostrar una
        // moneda distinta de la que dice el código.
        assert_eq!(render_amount(value, Some("EUR")), "€1,200.00 EUR");
        // Un código sin símbolo conocido en el motor se muestra tal cual,
        // sin inventar ningún signo.
        assert_eq!(render_amount(value, Some("CHF")), "1,200.00 CHF");
        // Sin moneda, el número no se presenta como si fuera dinero.
        assert_eq!(render_amount(value, None), "1,200");
    }

    #[test]
    fn render_amount_never_invents_mxn_for_an_unknown_currency() {
        let value = Decimal::from_f64(50.0).unwrap();
        let rendered = render_amount(value, None);
        assert!(!rendered.contains("MXN"));
        assert_eq!(rendered, "50");
    }

    #[test]
    fn currencies_never_share_an_accumulator() {
        let operands = [("MXN", 100.0), ("USD", 40.0), ("MXN", 200.0)]
            .into_iter()
            .enumerate()
            .map(|(index, (currency, amount))| Operand {
                document_id: index as i64,
                numeric: Some(amount),
                currency: Some(currency.into()),
                group: None,
                evidence: sample_evidence(index as i64),
            })
            .collect::<Vec<_>>();
        let rows = compute(Operation::Sum, &operands);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].currency.as_deref(), Some("MXN"));
        assert_eq!(rows[0].value.render(), "300");
        assert_eq!(rows[0].value_count, 2);
        assert_eq!(rows[1].currency.as_deref(), Some("USD"));
        assert_eq!(rows[1].value.render(), "40");
    }

    #[test]
    fn extremes_report_the_value_and_how_many_were_examined() {
        let operands = [3.0, 9.5, 1.25]
            .into_iter()
            .enumerate()
            .map(|(index, amount)| Operand {
                document_id: index as i64,
                numeric: Some(amount),
                currency: None,
                group: None,
                evidence: sample_evidence(index as i64),
            })
            .collect::<Vec<_>>();
        let maximum = compute(Operation::Maximum, &operands);
        assert_eq!(maximum[0].value.render(), "9.50");
        assert_eq!(maximum[0].value_count, 3);
        let minimum = compute(Operation::Minimum, &operands);
        assert_eq!(minimum[0].value.render(), "1.25");
    }

    #[test]
    fn a_table_with_a_missing_field_or_mixed_currencies_is_never_fully_verified() {
        let with_data = |currency: &str, value: f64| Bucket {
            group: None,
            currency: Some(currency.to_owned()),
            value: Decimal::from_f64(value).unwrap(),
            value_count: 1,
            document_ids: BTreeSet::from([1]),
            evidence: vec![sample_evidence(1)],
            has_unreliable_evidence: false,
        };
        // Campo A: una sola moneda, con datos.
        let campo_a = ("Campo A".to_owned(), vec![with_data("MXN", 500.0)]);
        // Campo B: sin ningún valor en el alcance — nunca se inventa un cero.
        let campo_b = ("Campo B".to_owned(), Vec::<Bucket>::new());
        // Campo C: el propio campo mezcla monedas distintas.
        let campo_c = (
            "Campo C".to_owned(),
            vec![with_data("MXN", 100.0), with_data("USD", 40.0)],
        );

        assert!(multi_field_is_fully_verified(&[campo_a.clone()]));
        assert!(!multi_field_is_fully_verified(&[campo_a.clone(), campo_b]));
        assert!(!multi_field_is_fully_verified(&[campo_a, campo_c]));
    }

    fn row_operand(document_id: i64, amount: f64, currency: Option<&str>) -> Operand {
        Operand {
            document_id,
            numeric: Some(amount),
            currency: currency.map(str::to_owned),
            group: None,
            evidence: sample_evidence(document_id),
        }
    }

    #[test]
    fn multiplying_two_fields_computes_row_by_row_and_sums_the_total() {
        // Cantidad (sin moneda) × Precio unitario (con moneda), documento por
        // documento: nunca multiplicar el total de Cantidad por el total de
        // Precio unitario, que sería una cifra distinta.
        let cantidad = [(1, 4.0), (2, 2.0), (3, 3.0)]
            .map(|(id, amount)| row_operand(id, amount, None));
        let precio = [(1, 125.0), (2, 150.0), (3, 10.0)]
            .map(|(id, amount)| row_operand(id, amount, Some("MXN")));
        let result = compute_row(RowOperation::Multiply, &cantidad, &precio, &[1, 2, 3]);
        assert!(result.skipped.is_empty());
        assert_eq!(result.breakdown.one_field_only, 0);
        assert_eq!(result.breakdown.neither_field, 0);
        assert_eq!(result.breakdown.calculated, 3);
        assert!(result.breakdown.is_exhaustive());
        assert_eq!(result.outcomes.len(), 3);
        let by_document = result
            .outcomes
            .iter()
            .map(|outcome| (outcome.document_id, outcome.value.render()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(by_document[&1], "500");
        assert_eq!(by_document[&2], "300");
        assert_eq!(by_document[&3], "30");
        let total = result
            .outcomes
            .iter()
            .fold(Decimal::ZERO, |acc, outcome| acc.add(outcome.value));
        // 500 + 300 + 30 = 830, nunca 9 × 285 (los totales globales de cada
        // campo multiplicados entre sí).
        assert_eq!(total.render(), "830");
        assert_eq!(result.outcomes[0].currency.as_deref(), Some("MXN"));
    }

    #[test]
    fn subtracting_two_fields_requires_the_same_currency() {
        let monto_a = [row_operand(1, 500.0, Some("MXN")), row_operand(2, 700.0, Some("MXN"))];
        let monto_b_same = [row_operand(1, 100.0, Some("MXN"))];
        let ok = compute_row(RowOperation::Subtract, &monto_a, &monto_b_same, &[1, 2]);
        assert_eq!(ok.outcomes.len(), 1);
        assert_eq!(ok.outcomes[0].value.render(), "400");
        assert_eq!(
            ok.breakdown.one_field_only, 1,
            "el documento 2 no tenía Monto B"
        );
        assert!(ok.breakdown.is_exhaustive());

        let monto_b_other_currency = [row_operand(1, 100.0, Some("USD"))];
        let incompatible = compute_row(
            RowOperation::Subtract,
            &monto_a,
            &monto_b_other_currency,
            &[1, 2],
        );
        assert!(incompatible.outcomes.is_empty());
        assert_eq!(incompatible.skipped.len(), 1);
        assert_eq!(incompatible.skipped[0].issue, RowIssue::IncompatibleUnits);
        assert_eq!(incompatible.breakdown.incompatible_units, 1);
        assert!(incompatible.breakdown.is_exhaustive());
    }

    #[test]
    fn dividing_two_fields_never_produces_zero_or_infinity_on_a_zero_divisor() {
        let dividend = [row_operand(1, 100.0, None), row_operand(2, 50.0, None)];
        let divisor = [row_operand(1, 4.0, None), row_operand(2, 0.0, None)];
        let result = compute_row(RowOperation::Divide, &dividend, &divisor, &[1, 2]);
        assert_eq!(result.outcomes.len(), 1);
        assert_eq!(result.outcomes[0].document_id, 1);
        assert_eq!(result.outcomes[0].value.render(), "25");
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].document_id, 2);
        assert_eq!(result.skipped[0].issue, RowIssue::DivisionByZero);
        assert_eq!(result.breakdown.division_by_zero, 1);
        assert!(result.breakdown.is_exhaustive());
    }

    #[test]
    fn a_document_with_both_fields_but_a_non_numeric_value_is_tracked_not_dropped() {
        // Documento 2 tiene los dos campos, pero «Cantidad» llegó como texto
        // no numérico («N/A»): antes desaparecía de outcomes, skipped y
        // unmatched_documents a la vez, y el total de documentos examinados
        // no cuadraba con lo que la respuesta afirmaba haber revisado.
        let invalid_left = Operand {
            document_id: 2,
            numeric: None,
            currency: None,
            group: None,
            evidence: sample_evidence(2),
        };
        let cantidad = [row_operand(1, 4.0, None), invalid_left];
        let precio = [row_operand(1, 125.0, Some("MXN")), row_operand(2, 10.0, Some("MXN"))];
        let result = compute_row(RowOperation::Multiply, &cantidad, &precio, &[1, 2]);
        assert_eq!(result.outcomes.len(), 1);
        assert_eq!(result.outcomes[0].document_id, 1);
        assert_eq!(result.skipped.len(), 1);
        assert_eq!(result.skipped[0].document_id, 2);
        assert_eq!(result.skipped[0].issue, RowIssue::InvalidValue);
        assert_eq!(
            result.breakdown.one_field_only, 0,
            "el documento 2 sí tenía los dos campos: no es lo mismo que un campo ausente"
        );
        assert_eq!(result.breakdown.invalid_value, 1);
        assert!(result.breakdown.is_exhaustive());
    }

    /// El caso que se escapaba: documentos del alcance que no traen NINGUNO
    /// de los dos campos. No aparecen en `left` ni en `right`, así que antes
    /// no se contaban en ninguna categoría y la respuesta podía declarar
    /// 600 documentos de alcance, 140 calculados y ninguna explicación para
    /// los 460 restantes — presentándose además como verificada.
    #[test]
    fn documents_with_neither_field_are_counted_instead_of_vanishing() {
        let cantidad = [row_operand(1, 4.0, None), row_operand(2, 2.0, None)];
        let precio = [
            row_operand(1, 125.0, Some("MXN")),
            row_operand(2, 150.0, Some("MXN")),
        ];
        // El alcance trae 6 documentos; sólo 2 tienen los dos campos, 1 tiene
        // uno solo y 3 no tienen ninguno.
        let with_one_field = [row_operand(3, 9.0, None)];
        let cantidad_con_suelto = [cantidad[0].clone(), cantidad[1].clone(), with_one_field[0].clone()];
        let result = compute_row(
            RowOperation::Multiply,
            &cantidad_con_suelto,
            &precio,
            &[1, 2, 3, 4, 5, 6],
        );
        assert_eq!(result.breakdown.scope_documents, 6);
        assert_eq!(result.breakdown.calculated, 2);
        assert_eq!(result.breakdown.one_field_only, 1, "el documento 3");
        assert_eq!(
            result.breakdown.neither_field, 3,
            "los documentos 4, 5 y 6 no tienen ninguno de los dos campos"
        );
        assert_eq!(result.breakdown.excluded(), 4);
        assert!(
            result.breakdown.is_exhaustive(),
            "6 = 2 calculados + 4 excluidos: {:?}",
            result.breakdown
        );
    }

    /// Un documento con dos valores del mismo campo produce dos resultados,
    /// pero sigue siendo un solo documento del alcance: si el reparto contara
    /// operandos en vez de documentos, el invariante se rompería.
    #[test]
    fn a_document_with_two_values_of_one_field_still_counts_once() {
        let cantidad = [row_operand(1, 4.0, None), row_operand(1, 6.0, None)];
        let precio = [row_operand(1, 10.0, Some("MXN"))];
        let result = compute_row(RowOperation::Multiply, &cantidad, &precio, &[1, 2]);
        assert_eq!(result.outcomes.len(), 2, "dos valores, dos productos");
        assert_eq!(
            result.breakdown.calculated, 1,
            "pero un solo documento calculado"
        );
        assert_eq!(result.breakdown.neither_field, 1, "el documento 2");
        assert!(result.breakdown.is_exhaustive());
    }

    #[test]
    fn dividing_money_by_money_of_the_same_currency_is_a_dimensionless_ratio() {
        let money = [row_operand(1, 500.0, Some("MXN"))];
        let other_money = [row_operand(1, 250.0, Some("MXN"))];
        let result = compute_row(RowOperation::Divide, &money, &other_money, &[1]);
        assert_eq!(result.outcomes[0].value.render(), "2");
        assert_eq!(result.outcomes[0].currency, None);
    }

    fn sample_evidence(document_id: i64) -> Evidence {
        Evidence {
            id: format!("value-{document_id}"),
            document_id,
            path: format!("/tmp/doc-{document_id}.txt"),
            origin: String::new(),
            location: "línea 1".into(),
            excerpt: "valor".into(),
            normalized_value: None,
            value: None,
            matched: None,
            field: None,
            match_kind: "campo".into(),
            reliable: true,
            ocr_status: None,
            ocr_confidence: None,
            confidence: None,
        }
    }
}
