import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

/*
 * Corpus sintéticos para evaluar recuperación documental local. No contienen
 * personas, empresas, operaciones ni documentos legales reales.
 */

const ROOT = process.cwd();
const YEAR = 2026;

const people = [
  'Adriana Salgado Nieto', 'Bruno Castañeda Mora', 'Claudia Rentería Vela',
  'Daniel Ibarra Cruz', 'Elena Ponce Salas', 'Felipe Navarro Arias',
  'Gabriela Solís Lara', 'Héctor Zamora Ríos', 'Inés Beltrán Soto',
  'Javier Montaño Díaz', 'Karla Figueroa León', 'Luis Trejo Paredes',
  'Mariana Quintana Gil', 'Nicolás Arce Fuentes', 'Olivia Serrano Vega',
  'Pablo Ledesma Ruiz', 'Renata Valdés Cota', 'Sergio Peralta Luna',
];

const cities = [
  ['Ciudad de México', 'Ciudad de México'], ['Guadalajara', 'Jalisco'],
  ['Monterrey', 'Nuevo León'], ['Puebla', 'Puebla'], ['Querétaro', 'Querétaro'],
  ['Mérida', 'Yucatán'], ['León', 'Guanajuato'], ['Toluca', 'Estado de México'],
];

function dateFor(index, offset = 0) {
  const date = new Date(Date.UTC(YEAR, 0, 5 + index * 3 + offset));
  return date.toISOString().slice(0, 10);
}

function money(amount, currency = 'MXN') {
  return new Intl.NumberFormat('es-MX', {
    style: 'currency', currency, maximumFractionDigits: 2,
  }).format(amount);
}

function slug(value) {
  return value
    .normalize('NFD').replace(/[\u0300-\u036f]/g, '')
    .replace(/[^a-zA-Z0-9]+/g, '-').replace(/(^-|-$)/g, '').toLowerCase();
}

function common(ctx, category) {
  return `
Folio: ${ctx.folio}
Tipo de documento: ${category.type}
Área responsable: ${category.area}
Estado: ${category.statuses[ctx.index % category.statuses.length]}
Empresa: ${ctx.spec.company}
Ciudad base: ${ctx.city[0]}
Estado de operación: ${ctx.city[1]}
Fecha de registro: ${ctx.date}
Responsable interno: ${ctx.owner}
Clasificación de datos: Uso interno — datos sintéticos de prueba
`;
}

function context(spec, category, index, localIndex) {
  const city = cities[(index * 3 + localIndex) % cities.length];
  const client = people[(index * 5 + 1) % people.length];
  const owner = people[(index * 7 + 4) % people.length];
  return {
    spec, category, index, localIndex, city, client, owner,
    date: dateFor(index),
    folio: `${spec.prefix}-${String(YEAR).slice(-2)}-${String(index + 1).padStart(4, '0')}`,
    amount: spec.amountBase + (index % 17) * spec.amountStep,
  };
}

function notaryBody(ctx, category) {
  const property = [
    'departamento habitacional', 'local comercial', 'predio rústico', 'oficina corporativa',
    'bodega industrial', 'casa habitación',
  ][ctx.index % 6];
  const grantor = people[(ctx.index + 2) % people.length];
  const book = 180 + (ctx.index % 34);
  const value = ctx.amount + 650000;
  const categoryDetails = {
    escrituras: `Acto jurídico: Compraventa de ${property}\nOtorgante: ${grantor}\nAdquirente: ${ctx.client}\nValor declarado: ${money(value)}\nLibro de protocolo: ${book}\nInstrumento: ${ctx.folio}-EP`,
    poderes: `Poderdante: ${grantor}\nApoderado: ${ctx.client}\nFacultades conferidas: pleitos y cobranzas, actos de administración\nVigencia indicada: ${dateFor(ctx.index, 730)}\nInstrumento: ${ctx.folio}-PO`,
    testamentos: `Testador: ${ctx.client}\nTipo de disposición: testamento público abierto\nAlbacea designado: ${grantor}\nLibro de protocolo: ${book}\nInstrumento: ${ctx.folio}-TE`,
    certificaciones: `Solicitante: ${ctx.client}\nDocumento certificado: copia de instrumento protocolizado\nNúmero de copias: ${1 + (ctx.index % 4)}\nInstrumento relacionado: ${ctx.folio}-CP`,
    avisos: `Operación reportable: ${category.type}\nParte relacionada: ${ctx.client}\nMonto de referencia: ${money(value)}\nFecha de aviso: ${dateFor(ctx.index, 2)}\nExpediente de cumplimiento: CMP-${ctx.folio}`,
    expedientes: `Solicitante: ${ctx.client}\nAsunto: integración documental previa a instrumento\nDocumentos verificados: identificación, constancia fiscal y comprobante de domicilio\nExpediente: EXP-${ctx.folio}`,
    facturas: `Receptor: ${ctx.client}\nConcepto facturado: honorarios notariales y derechos estimados\nSubtotal: ${money(ctx.amount)}\nIVA trasladado: ${money(Math.round(ctx.amount * 0.16))}\nTotal facturado: ${money(Math.round(ctx.amount * 1.16))}`,
  };
  return `# ${category.type}\n${common(ctx, category)}${categoryDetails[category.key]}

## Antecedentes y revisión documental

Se integró un expediente de trabajo con los documentos presentados por las personas comparecientes y con las referencias necesarias para preparar el instrumento. El personal revisó que los datos de identificación coincidieran entre sí, que la información proporcionada fuera legible y que las firmas o autorizaciones requeridas se solicitaran antes de cerrar el trámite. Cualquier inconsistencia se deja como observación pendiente y no se interpreta como confirmación de derechos, propiedad o capacidad jurídica.

La información de esta ficha es un resumen operativo. Las partes deben revisar el proyecto de instrumento, sus declaraciones y los importes antes de firmar. Cuando el asunto requiera consulta registral, fiscal, catastral o judicial, esa gestión se documenta en el expediente y se confirma con la autoridad o profesional competente; no se infiere a partir de una conversación o copia simple.

## Controles, privacidad y seguimiento

El acceso al expediente se limita al personal asignado y a las personas con autorización acreditada. No se deben compartir identificaciones, domicilios, datos patrimoniales ni borradores con terceros sin fundamento y autorización aplicable. Los pagos se registran por concepto y comprobante; una instrucción de pago que provenga de alguien distinto a la parte acreditada se escala a cumplimiento antes de aceptarse.

Antes del cierre se verifica la lista de requisitos, el cálculo de honorarios y derechos, la fecha de firma y la conservación de los anexos. Este documento es ficticio y se creó exclusivamente para probar indexación, filtros, conteos, importes y recuperación de texto; no constituye una escritura, poder, testamento, certificación ni asesoría legal real.`;
}

function legalBody(ctx, category) {
  const counterparty = people[(ctx.index + 6) % people.length];
  const matter = [
    'incumplimiento contractual', 'arrendamiento comercial', 'responsabilidad civil',
    'cobranza mercantil', 'protección de datos', 'conflicto laboral',
  ][ctx.index % 6];
  const details = {
    asuntos: `Cliente: ${ctx.client}\nContraparte: ${counterparty}\nMateria: ${matter}\nNúmero de expediente: EXP-${ctx.folio}\nPretensión inicial: ${money(ctx.amount + 80000)}`,
    contratos: `Cliente: ${ctx.client}\nContraparte: ${counterparty}\nObjeto contractual: prestación de servicios profesionales\nImporte contractual: ${money(ctx.amount + 120000)}\nVigencia: ${ctx.date} a ${dateFor(ctx.index, 365)}`,
    escritos: `Promovente: ${ctx.client}\nAutoridad o destinatario: Unidad de trámite correspondiente\nAsunto: ${matter}\nExpediente relacionado: EXP-${ctx.folio}\nFecha límite interna: ${dateFor(ctx.index, 12)}`,
    dictamenes: `Solicitante: ${ctx.client}\nMateria analizada: ${matter}\nNivel de riesgo: ${['Bajo', 'Medio', 'Alto'][ctx.index % 3]}\nExpediente relacionado: EXP-${ctx.folio}`,
    cumplimiento: `Cliente evaluado: ${ctx.client}\nControl revisado: identificación, conflicto de interés y autorización\nResultado de revisión: ${ctx.index % 5 === 0 ? 'Observación abierta' : 'Sin hallazgos críticos'}\nExpediente: CMP-${ctx.folio}`,
    tiempos: `Cliente: ${ctx.client}\nProfesional responsable: ${ctx.owner}\nHoras registradas: ${2 + (ctx.index % 9)}\nTarifa de referencia: ${money(1800 + (ctx.index % 4) * 350)}\nExpediente relacionado: EXP-${ctx.folio}`,
    facturas: `Receptor: ${ctx.client}\nConcepto facturado: servicios jurídicos profesionales\nSubtotal: ${money(ctx.amount)}\nIVA trasladado: ${money(Math.round(ctx.amount * 0.16))}\nTotal facturado: ${money(Math.round(ctx.amount * 1.16))}`,
  };
  return `# ${category.type}\n${common(ctx, category)}${details[category.key]}

## Alcance profesional y hechos disponibles

El asunto se abrió con la información que el cliente entregó en la entrevista inicial y con los documentos identificados en el expediente. La descripción de hechos se conserva como declaración de quien la proporciona mientras no exista evidencia independiente que la confirme. El equipo responsable debe señalar documentos faltantes, plazos relevantes y riesgos de conservación de evidencia antes de recomendar una actuación.

La estrategia, escritos y comunicaciones externas requieren revisión de la persona responsable del asunto. No se debe prometer un resultado, presentar información incompleta como hecho confirmado ni tomar una decisión procesal únicamente por presión de calendario. Las instrucciones del cliente que modifiquen alcance, presupuesto o postura deben quedar documentadas en el expediente.

## Confidencialidad, conflictos y próximos pasos

La información del cliente y de la contraparte se utiliza exclusivamente para el encargo autorizado. Cualquier conflicto de interés, solicitud de un tercero o intento de obtener documentos sin autorización se reporta al responsable de cumplimiento. Los archivos se comparten mediante los canales definidos por el despacho y se evita incluir datos sensibles en mensajes sin protección.

El siguiente paso consiste en confirmar los documentos pendientes, revisar el calendario de actuaciones y obtener la autorización correspondiente. Este documento es enteramente sintético; no es un expediente judicial, contrato, dictamen o consejo jurídico utilizable.`;
}

function hardwareBody(ctx, category) {
  const sku = `FER-${String(1000 + ctx.index).padStart(5, '0')}`;
  const item = [
    'taladro inalámbrico', 'cemento gris', 'tubo de cobre', 'pintura acrílica',
    'cerradura de seguridad', 'cable eléctrico', 'llave mezcladora', 'guantes de protección',
  ][ctx.index % 8];
  const supplier = ['Suministros del Centro', 'Materiales Norte', 'Herramientas Atlas', 'Acabados Regionales'][ctx.index % 4];
  const details = {
    ventas: `Cliente: ${ctx.client}\nProducto principal: ${item}\nSKU: ${sku}\nCantidad vendida: ${2 + (ctx.index % 16)}\nSubtotal: ${money(ctx.amount)}\nIVA trasladado: ${money(Math.round(ctx.amount * 0.16))}\nTotal facturado: ${money(Math.round(ctx.amount * 1.16))}`,
    compras: `Proveedor: ${supplier}\nProducto solicitado: ${item}\nSKU: ${sku}\nCantidad solicitada: ${20 + (ctx.index % 80)}\nImporte de compra: ${money(ctx.amount + 5000)}\nFecha estimada de entrega: ${dateFor(ctx.index, 8)}`,
    inventario: `Producto: ${item}\nSKU: ${sku}\nExistencia física: ${15 + (ctx.index % 140)}\nPunto de reorden: ${10 + (ctx.index % 24)}\nUbicación de almacén: Pasillo ${1 + (ctx.index % 8)}, módulo ${String.fromCharCode(65 + (ctx.index % 5))}`,
    proveedores: `Proveedor: ${supplier}\nCategoría de suministro: construcción y mantenimiento\nContacto operativo: ${people[(ctx.index + 3) % people.length]}\nLímite de crédito: ${money(ctx.amount + 25000)}\nFecha de evaluación: ${ctx.date}`,
    entregas: `Cliente: ${ctx.client}\nProducto principal: ${item}\nSKU: ${sku}\nCantidad entregada: ${2 + (ctx.index % 30)}\nDirección de entrega: zona comercial de ${ctx.city[0]}\nReferencia de venta: VTA-${ctx.folio}`,
    seguridad: `Área inspeccionada: almacén y piso de ventas\nRiesgo identificado: ${['carga manual', 'orden y limpieza', 'herramienta eléctrica', 'señalización'][ctx.index % 4]}\nAcción correctiva: capacitación y verificación de equipo\nFecha compromiso: ${dateFor(ctx.index, 15)}`,
    facturas: `Receptor: ${ctx.client}\nConcepto facturado: venta de materiales y herramientas\nSubtotal: ${money(ctx.amount)}\nIVA trasladado: ${money(Math.round(ctx.amount * 0.16))}\nTotal facturado: ${money(Math.round(ctx.amount * 1.16))}`,
  };
  return `# ${category.type}\n${common(ctx, category)}${details[category.key]}

## Operación comercial

El registro documenta una actividad de venta, abastecimiento, inventario o seguridad realizada en la sucursal indicada. El personal validó la descripción del producto, unidad de medida y cantidad antes de confirmar la operación. Cuando existe diferencia entre inventario físico, sistema y comprobante, se genera una observación para revisión; no se ajusta la existencia sin referencia y autorización.

Los precios, descuentos y tiempos de entrega deben comunicarse por los canales autorizados. Una entrega depende de disponibilidad, dirección validada y condiciones seguras de descarga. Los productos con manejo especial se preparan conforme a sus indicaciones de seguridad y no se sustituyen por equivalentes sin avisar al cliente o responsable de compra.

## Control y seguimiento

Las devoluciones, faltantes, daños de transporte o cambios de precio requieren evidencia y aprobación del área responsable. El acceso a descuentos, crédito y datos de clientes se limita al personal autorizado. Este archivo es una simulación para pruebas; no acredita una venta, compra, inventario, factura ni condición comercial real.`;
}

function insuranceBody(ctx, category) {
  const insured = people[(ctx.index + 7) % people.length];
  const product = ['auto particular', 'hogar integral', 'vida temporal', 'gastos médicos', 'negocio protegido'][ctx.index % 5];
  const details = {
    polizas: `Asegurado: ${insured}\nContratante: ${ctx.client}\nRamo: ${product}\nNúmero de póliza: POL-${ctx.folio}\nSuma asegurada: ${money(ctx.amount + 350000)}\nPrima anual: ${money(Math.round(ctx.amount * 0.12))}\nVigencia: ${ctx.date} a ${dateFor(ctx.index, 365)}`,
    siniestros: `Asegurado: ${insured}\nNúmero de siniestro: SIN-${ctx.folio}\nRamo: ${product}\nFecha del evento: ${dateFor(ctx.index, -2)}\nMonto reclamado: ${money(ctx.amount + 45000)}\nAjustador asignado: ${ctx.owner}`,
    suscripcion: `Solicitante: ${ctx.client}\nRamo solicitado: ${product}\nNivel de riesgo: ${['Bajo', 'Medio', 'Alto'][ctx.index % 3]}\nPrima cotizada: ${money(Math.round(ctx.amount * 0.12))}\nVigencia propuesta: ${dateFor(ctx.index, 365)}`,
    pagos: `Contratante: ${ctx.client}\nReferencia de póliza: POL-${ctx.folio}\nImporte recibido: ${money(Math.round(ctx.amount * 0.12))}\nMedio de pago: transferencia bancaria\nFecha de aplicación: ${ctx.date}`,
    renovaciones: `Asegurado: ${insured}\nNúmero de póliza: POL-${ctx.folio}\nPrima de renovación: ${money(Math.round(ctx.amount * 0.13))}\nFecha límite de renovación: ${dateFor(ctx.index, 30)}\nEstatus de contacto: ${ctx.index % 4 === 0 ? 'Pendiente de respuesta' : 'Propuesta enviada'}`,
    agentes: `Agente: ${ctx.owner}\nClave de agente: AG-${String(100 + ctx.index).padStart(4, '0')}\nRamo atendido: ${product}\nCiudad de operación: ${ctx.city[0]}\nCapacitación vigente hasta: ${dateFor(ctx.index, 180)}`,
    cumplimiento: `Cliente evaluado: ${ctx.client}\nControl revisado: identificación, aviso de privacidad y origen de pago\nResultado: ${ctx.index % 6 === 0 ? 'Validación adicional requerida' : 'Validación completada'}\nExpediente: CMP-${ctx.folio}`,
  };
  return `# ${category.type}\n${common(ctx, category)}${details[category.key]}

## Información de la operación

La ficha se integra con los datos proporcionados por el solicitante, asegurado o tercero relacionado y sirve para coordinar la atención del trámite. La cobertura, exclusiones, deducibles, primas y vigencias aplicables son únicamente los que consten en la póliza o documentación autorizada. El personal no debe confirmar cobertura definitiva ni prometer pago de un siniestro antes de la validación correspondiente.

Para siniestros se solicita conservar evidencia, comunicar hechos de manera completa y evitar reparaciones o disposiciones que impidan la inspección cuando ésta sea necesaria. Para pagos o renovaciones se revisa que la referencia coincida con el expediente y que el medio de pago sea aceptable conforme a las políticas internas.

## Privacidad y seguimiento

Los datos personales, médicos, financieros y de bienes tienen acceso restringido. Una petición hecha por un tercero requiere validar identidad, facultades y alcance antes de revelar información. Las observaciones de riesgo, cobro o documentación se registran para su seguimiento sin alterar los documentos de origen.

Este documento es ficticio y se creó para pruebas de recuperación. No es una póliza, aviso de siniestro, cotización, pago, renovación ni autorización de seguros real.`;
}

function restaurantBody(ctx, category) {
  const diner = people[(ctx.index + 8) % people.length];
  const dish = ['mole poblano', 'tacos de pescado', 'risotto de hongos', 'ensalada mediterránea', 'salmón a la plancha', 'pasta al pesto'][ctx.index % 6];
  const supplier = ['Huerto Verde', 'Carnes del Valle', 'Pescados del Puerto', 'Panadería Central'][ctx.index % 4];
  const details = {
    reservaciones: `Cliente titular: ${diner}\nFecha de reserva: ${dateFor(ctx.index, 4)}\nHora: ${String(13 + (ctx.index % 8)).padStart(2, '0')}:00\nNúmero de comensales: ${2 + (ctx.index % 10)}\nMesa asignada: ${1 + (ctx.index % 22)}\nPreferencias alimentarias: ${ctx.index % 3 === 0 ? 'Sin nueces' : 'Sin restricciones reportadas'}`,
    comandas: `Cliente o mesa: ${1 + (ctx.index % 22)}\nPlatillo principal: ${dish}\nNúmero de comensales: ${2 + (ctx.index % 8)}\nImporte de consumo: ${money(ctx.amount)}\nMesero responsable: ${ctx.owner}\nReferencia de reservación: RES-${ctx.folio}`,
    proveedores: `Proveedor: ${supplier}\nProducto recibido: ${dish}\nLote: LOT-${ctx.folio}\nCantidad recibida: ${8 + (ctx.index % 45)} kg\nCosto de compra: ${money(ctx.amount)}\nFecha de caducidad: ${dateFor(ctx.index, 10 + (ctx.index % 20))}`,
    personal: `Colaborador: ${ctx.owner}\nPuesto: ${['Cocinero', 'Mesero', 'Hostess', 'Gerente de turno', 'Lavaloza'][ctx.index % 5]}\nTurno: ${ctx.index % 2 === 0 ? 'Matutino' : 'Vespertino'}\nCapacitación de higiene vigente hasta: ${dateFor(ctx.index, 180)}\nSupervisor: ${people[(ctx.index + 3) % people.length]}`,
    sanidad: `Área inspeccionada: ${['cocina caliente', 'cámara fría', 'barra', 'almacén seco'][ctx.index % 4]}\nControl verificado: temperatura, limpieza y rotación\nResultado: ${ctx.index % 7 === 0 ? 'Acción correctiva abierta' : 'Conforme'}\nFecha compromiso: ${dateFor(ctx.index, 3)}`,
    facturas: `Receptor: ${diner}\nConcepto facturado: consumo de alimentos y bebidas\nSubtotal: ${money(ctx.amount)}\nIVA trasladado: ${money(Math.round(ctx.amount * 0.16))}\nTotal facturado: ${money(Math.round(ctx.amount * 1.16))}`,
    incidentes: `Cliente involucrado: ${diner}\nTipo de incidente: ${['queja de servicio', 'alergia reportada', 'objeto extraviado', 'derrame en salón'][ctx.index % 4]}\nHora de reporte: ${String(12 + (ctx.index % 9)).padStart(2, '0')}:20\nAcción inmediata: atención del gerente y registro de evidencia\nFolio de seguimiento: INC-${ctx.folio}`,
  };
  return `# ${category.type}\n${common(ctx, category)}${details[category.key]}

## Registro operativo

El presente registro documenta una actividad de atención, cocina, abastecimiento, higiene, facturación o seguimiento de incidente. La información debe verificarse contra la comanda, reservación, recepción de insumos o bitácora que corresponda. El equipo comunica restricciones alimentarias y observaciones de servicio al personal involucrado, sin exponer datos personales innecesarios.

Los alimentos se preparan y conservan conforme a los controles internos de higiene, temperatura y rotación. Ante una incidencia, el responsable de turno debe priorizar la atención de la persona, preservar información relevante y escalar el caso cuando se requiera. No se debe alterar una bitácora ni desechar evidencia de una queja antes de que se documente su seguimiento.

## Datos y cierre

Las preferencias de clientes, contactos y datos de facturación se usan sólo para la reserva, servicio, comprobación y seguimiento autorizado. Las devoluciones, cortesías y ajustes requieren aprobación conforme a las políticas del restaurante. Este archivo es sintético y sólo existe para probar Omega; no representa una reservación, consumo, factura, control sanitario o incidente real.`;
}

const specs = [
  {
    key: 'notaria', folder: 'corpus-prueba-notaria', title: 'Notaría', prefix: 'NOT',
    company: 'Notaría Modelo 18, S.C.', amountBase: 18500, amountStep: 725,
    body: notaryBody,
    categories: [
      ['01_escrituras', 'escrituras', 'Escritura pública', 'Protocolo', 22, ['En preparación', 'Firmada', 'En inscripción']],
      ['02_poderes', 'poderes', 'Poder notarial', 'Protocolo', 14, ['En revisión', 'Otorgado']],
      ['03_testamentos', 'testamentos', 'Testamento público abierto', 'Protocolo', 12, ['Programado', 'Formalizado']],
      ['04_certificaciones', 'certificaciones', 'Certificación de documento', 'Archivo notarial', 12, ['Solicitada', 'Entregada']],
      ['05_avisos_cumplimiento', 'avisos', 'Aviso de cumplimiento', 'Cumplimiento', 12, ['En validación', 'Presentado']],
      ['06_expedientes', 'expedientes', 'Expediente de compareciente', 'Gestoría', 14, ['Incompleto', 'Integrado']],
      ['07_facturas', 'facturas', 'Factura de servicios notariales', 'Administración', 14, ['Emitida', 'Pagada']],
    ],
  },
  {
    key: 'despacho-legal', folder: 'corpus-prueba-despacho-legal', title: 'Despacho legal', prefix: 'DLG',
    company: 'Lumen Juris Consultores, S.C.', amountBase: 21600, amountStep: 850,
    body: legalBody,
    categories: [
      ['01_asuntos', 'asuntos', 'Expediente de asunto legal', 'Litigio y consultoría', 22, ['Abierto', 'En análisis', 'En seguimiento']],
      ['02_contratos', 'contratos', 'Contrato revisado', 'Corporativo', 16, ['En revisión', 'Aprobado']],
      ['03_escritos', 'escritos', 'Escrito jurídico', 'Litigio', 14, ['Borrador', 'Presentado']],
      ['04_dictamenes', 'dictamenes', 'Dictamen legal', 'Consultoría', 12, ['En elaboración', 'Entregado']],
      ['05_cumplimiento', 'cumplimiento', 'Revisión de cumplimiento', 'Cumplimiento', 12, ['Abierto', 'Cerrado']],
      ['06_tiempos_honorarios', 'tiempos', 'Registro de tiempo profesional', 'Administración', 12, ['Registrado', 'Aprobado']],
      ['07_facturas', 'facturas', 'Factura de servicios jurídicos', 'Administración', 12, ['Emitida', 'Pagada']],
    ],
  },
  {
    key: 'ferreteria', folder: 'corpus-prueba-ferreteria', title: 'Ferretería', prefix: 'FER',
    company: 'Ferretería Punto Firme, S.A. de C.V.', amountBase: 3200, amountStep: 490,
    body: hardwareBody,
    categories: [
      ['01_ventas', 'ventas', 'Nota de venta', 'Ventas', 24, ['Cerrada', 'Facturada']],
      ['02_compras', 'compras', 'Orden de compra', 'Compras', 16, ['Solicitada', 'Recibida']],
      ['03_inventario', 'inventario', 'Registro de inventario', 'Almacén', 18, ['Disponible', 'Reorden requerido']],
      ['04_proveedores', 'proveedores', 'Expediente de proveedor', 'Compras', 10, ['Activo', 'En evaluación']],
      ['05_entregas', 'entregas', 'Remisión de entrega', 'Logística', 12, ['En ruta', 'Entregada']],
      ['06_seguridad', 'seguridad', 'Inspección de seguridad', 'Seguridad e higiene', 10, ['Abierta', 'Corregida']],
      ['07_facturas', 'facturas', 'Factura de materiales', 'Administración', 10, ['Emitida', 'Pagada']],
    ],
  },
  {
    key: 'seguros', folder: 'corpus-prueba-seguros', title: 'Oficina de seguros', prefix: 'SEG',
    company: 'Protección Integral Agencia de Seguros, S.A. de C.V.', amountBase: 14500, amountStep: 910,
    body: insuranceBody,
    categories: [
      ['01_polizas', 'polizas', 'Expediente de póliza', 'Operación de pólizas', 24, ['Vigente', 'Pendiente de emisión']],
      ['02_siniestros', 'siniestros', 'Aviso de siniestro', 'Siniestros', 16, ['En revisión', 'Documentación pendiente', 'Cerrado']],
      ['03_suscripcion', 'suscripcion', 'Solicitud de suscripción', 'Suscripción', 14, ['En análisis', 'Cotizada']],
      ['04_pagos', 'pagos', 'Registro de pago de prima', 'Cobranza', 12, ['Aplicado', 'Por conciliar']],
      ['05_renovaciones', 'renovaciones', 'Propuesta de renovación', 'Renovaciones', 12, ['Enviada', 'Pendiente de respuesta']],
      ['06_agentes', 'agentes', 'Expediente de agente', 'Red comercial', 10, ['Activo', 'Actualización pendiente']],
      ['07_cumplimiento', 'cumplimiento', 'Validación de cumplimiento', 'Cumplimiento', 12, ['Completada', 'Validación adicional requerida']],
    ],
  },
  {
    key: 'restaurante', folder: 'corpus-prueba-restaurante', title: 'Restaurante', prefix: 'RES',
    company: 'Casa Sazón Restaurante, S.A. de C.V.', amountBase: 950, amountStep: 180,
    body: restaurantBody,
    categories: [
      ['01_reservaciones', 'reservaciones', 'Reservación de mesa', 'Recepción', 22, ['Confirmada', 'Pendiente de confirmación']],
      ['02_comandas', 'comandas', 'Comanda de servicio', 'Piso y cocina', 22, ['Cerrada', 'En preparación']],
      ['03_proveedores', 'proveedores', 'Recepción de insumos', 'Compras', 14, ['Recibida', 'En revisión']],
      ['04_personal', 'personal', 'Expediente de personal', 'Recursos Humanos', 12, ['Activo', 'Actualización pendiente']],
      ['05_sanidad', 'sanidad', 'Bitácora de sanidad', 'Calidad e higiene', 12, ['Conforme', 'Acción correctiva abierta']],
      ['06_facturas', 'facturas', 'Factura de consumo', 'Administración', 10, ['Emitida', 'Pagada']],
      ['07_incidentes', 'incidentes', 'Registro de incidente', 'Gerencia', 8, ['Abierto', 'En seguimiento']],
    ],
  },
].map((spec) => ({
  ...spec,
  categories: spec.categories.map(([folder, key, type, area, count, statuses]) => ({ folder, key, type, area, count, statuses })),
}));

async function generateSpec(spec) {
  const root = join(ROOT, spec.folder);
  await mkdir(root, { recursive: true });
  let index = 0;
  for (const category of spec.categories) {
    const dir = join(root, category.folder);
    await mkdir(dir, { recursive: true });
    for (let localIndex = 0; localIndex < category.count; localIndex += 1) {
      const ctx = context(spec, category, index, localIndex);
      const filename = `${String(localIndex + 1).padStart(3, '0')}-${slug(category.type)}-${ctx.folio}.md`;
      await writeFile(join(dir, filename), `${spec.body(ctx, category)}\n`, 'utf8');
      index += 1;
    }
  }
  return { folder: spec.folder, count: index, categories: spec.categories };
}

const results = await Promise.all(specs.map(generateSpec));
const total = results.reduce((sum, item) => sum + item.count, 0);
console.log(`Generados ${total} documentos sintéticos en ${results.length} corpus.`);
for (const result of results) console.log(`- ${result.folder}: ${result.count}`);
