import { mkdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const root = join(process.cwd(), 'corpus-prueba-agencia-yates');

const cities = [
  ['Cabo San Lucas', 'Baja California Sur', 'Marina Cabo San Lucas', 'Mar de Cortés'],
  ['La Paz', 'Baja California Sur', 'Marina CostaBaja', 'Bahía de La Paz'],
  ['Puerto Vallarta', 'Jalisco', 'Marina Vallarta', 'Bahía de Banderas'],
  ['Cancún', 'Quintana Roo', 'Marina Puerto Cancún', 'Caribe Mexicano'],
  ['Playa del Carmen', 'Quintana Roo', 'Terminal Náutica', 'Caribe Mexicano'],
  ['Cozumel', 'Quintana Roo', 'Marina Fonatur', 'Canal de Cozumel'],
  ['Mazatlán', 'Sinaloa', 'Marina Mazatlán', 'Océano Pacífico'],
  ['Acapulco', 'Guerrero', 'Marina Acapulco', 'Bahía de Santa Lucía'],
  ['Ixtapa-Zihuatanejo', 'Guerrero', 'Marina Ixtapa', 'Pacífico Sur'],
  ['Ensenada', 'Baja California', 'Marina Baja Naval', 'Bahía de Todos Santos'],
  ['Huatulco', 'Oaxaca', 'Marina Chahué', 'Bahías de Huatulco'],
  ['Veracruz', 'Veracruz', 'Marina Veramar', 'Golfo de México'],
];

const people = [
  'Sofía Valdés Romero', 'Mateo Cárdenas Ruiz', 'Camila Ortega Luna', 'Diego Salvatierra Ponce',
  'Valeria Montaño Cruz', 'Emilio Rivas Solís', 'Luciana Figueroa Arias', 'Bruno Méndez Lara',
  'Renata Villaseñor Díaz', 'Tomás Beltrán Vega', 'Mariana Castañeda Gil', 'Julián Arce Mora',
  'Elena Quintana Ríos', 'Nicolás Peralta Soto', 'Paola Serrano Nieto', 'Andrés Ledesma Vela',
  'Clara Zamora Fuentes', 'Rodrigo Ibarra León', 'Mónica Trejo Navas', 'Gabriel Pineda Cota',
];

const yachtModels = [
  ['Azimut 55 Flybridge', 'motor', 55, 12, 'AZM'], ['Sunseeker Manhattan 66', 'motor', 66, 14, 'SUN'],
  ['Princess V50', 'motor', 50, 10, 'PRI'], ['Beneteau Oceanis 46.1', 'velero', 46, 9, 'BEN'],
  ['Lagoon 46', 'catamarán', 46, 12, 'LAG'], ['Sea Ray SLX 400', 'motor', 40, 12, 'SEA'],
  ['Ferretti 670', 'motor', 67, 14, 'FER'], ['Jeanneau DB/43', 'motor', 43, 10, 'JEA'],
  ['Fountaine Pajot Isla 40', 'catamarán', 40, 10, 'FPA'], ['Pershing 5X', 'motor', 54, 10, 'PER'],
];

const services = [
  'renta privada de medio día', 'renta privada de día completo', 'charter al atardecer',
  'traslado ejecutivo marítimo', 'experiencia de snorkel guiado', 'propuesta de compra de yate',
  'servicio corporativo con catering', 'salida de pesca deportiva', 'crucero de dos noches',
];

const vendors = [
  'Combustibles del Pacífico', 'Náutica Segura del Caribe', 'Astilleros Costa Norte', 'Provisiones Azul Profundo',
  'Seguros Horizonte Marino', 'Electrónica Ocean Link', 'Servicios Subacuáticos Arrecife', 'Lonas y Cabos del Golfo',
  'Catering Brisa Salada', 'Mecánica Diesel Bahía', 'Uniformes Marea Alta', 'Rescate Médico Costero',
];

function money(amount) {
  return new Intl.NumberFormat('es-MX', { style: 'currency', currency: 'MXN', maximumFractionDigits: 2 }).format(amount);
}

function dateFor(index, offset = 0) {
  const base = new Date(Date.UTC(2026, 0, 2 + index * 3 + offset));
  return base.toISOString().slice(0, 10);
}

function context(index) {
  const city = cities[index % cities.length];
  const yacht = yachtModels[index % yachtModels.length];
  const client = people[(index * 3 + 2) % people.length];
  const employee = people[(index * 7 + 5) % people.length];
  const folio = `MAY-${String(2026).slice(-2)}-${String(index + 1).padStart(4, '0')}`;
  return { index, city, yacht, client, employee, folio, date: dateFor(index), amount: 42000 + (index % 19) * 7350 };
}

function common(c, category, status, type) {
  const [city, state, marina, sea] = c.city;
  const [model, vesselType, feet, capacity, prefix] = c.yacht;
  return `
Folio: ${c.folio}-${prefix}
Tipo de documento: ${type}
Área responsable: ${category}
Estado: ${status}
Empresa: Mar Azul Charter & Sales, S.A. de C.V.
Ciudad base: ${city}
Estado de operación: ${state}
Marina: ${marina}
Zona de navegación: ${sea}
Embarcación: ${model}
Clase de embarcación: ${vesselType}
Eslora: ${feet} pies
Capacidad autorizada: ${capacity} pasajeros
Responsable interno: ${c.employee}
Fecha de registro: ${c.date}
Clasificación de datos: Uso interno — datos sintéticos de prueba
`;
}

function sales(c) {
  const price = 7800000 + (c.index % 12) * 925000;
  const deposit = Math.round(price * (0.08 + (c.index % 5) * 0.02));
  return `# Expediente de venta de embarcación\n${common(c, 'Ventas', c.index % 7 === 0 ? 'En negociación' : 'Prospecto calificado', 'Expediente comercial de venta')}
Cliente potencial: ${c.client}
Servicio solicitado: propuesta de compra de yate
Precio de lista: ${money(price)}
Anticipo propuesto: ${money(deposit)}
Moneda de referencia: MXN
Asesor comercial: ${c.employee}
Vigencia de propuesta: ${dateFor(c.index, 21)}

## Perfil y necesidad del cliente

El prospecto fue atendido mediante una videollamada de diagnóstico y una visita programada a ${c.city}. Indicó que busca una embarcación con operación familiar, posibilidad de recibir invitados y autonomía suficiente para travesías costeras. Se documentó que su decisión de compra depende de confirmar costos anuales, disponibilidad de amarre y el historial de mantenimiento antes de liberar cualquier anticipo. La conversación se registró como oportunidad de venta y no constituye asesoría fiscal, marítima o financiera.

La recomendación inicial fue ${c.yacht[0]}, considerando la capacidad de ${c.yacht[3]} personas, la eslora de ${c.yacht[2]} pies y la operación prevista en ${c.city[3]}. El asesor explicó que los precios pueden modificarse por tipo de cambio, traslado, impuestos, equipamiento seleccionado y condiciones de entrega. También se solicitó al cliente mantener la información de precio y disponibilidad con carácter confidencial mientras se prepara la cotización definitiva.

## Revisión técnica, documental y de entrega

Antes de firmar una orden de compra, Mar Azul deberá entregar al interesado una carpeta de revisión con matrícula o registro aplicable, comprobantes de propiedad disponibles, inventario de equipo de seguridad, bitácoras de servicio y constancias de pago que correspondan. La validación de gravámenes, situación registral y poderes de representación se asignará a un despacho externo elegido por el comprador. Ninguna representación verbal del equipo comercial sustituye la revisión legal independiente.

La propuesta contempla inspección previa a la entrega, prueba de navegación con capitán autorizado y un acta de aceptación. Cualquier hallazgo material en motores, generador, casco, sistemas eléctricos o electrónica se clasificará por criticidad y deberá resolverse o acordarse por escrito. Si el comprador requiere traslado a otra marina, se cotizarán tripulación, combustible, seguros y permisos de ruta en un anexo independiente.

## Cumplimiento y siguientes pasos

El expediente queda sujeto a verificación de identidad, beneficiario controlador cuando proceda, origen lícito de recursos y aceptación de aviso de privacidad. Los pagos superiores a los límites internos deben hacerse por transferencia desde una cuenta a nombre del comprador o de la sociedad acreditada; no se aceptan instrucciones de pago de terceros sin validación documental. El área de cumplimiento puede detener la operación si detecta inconsistencias.

La siguiente reunión está prevista para revisar la lista de opciones, el esquema de mantenimiento anual y la propuesta de amarre. El asesor deberá actualizar este documento con los acuerdos y conservar los correos de autorización en el CRM. Este expediente es ficticio y se creó exclusivamente para probar recuperación documental en Omega.`;
}

function reservation(c) {
  const passengers = 4 + (c.index % Math.max(2, c.yacht[3] - 3));
  const rate = c.amount;
  const deposit = Math.round(rate * 0.5);
  return `# Confirmación operativa de reserva\n${common(c, 'Reservas y charter', c.index % 13 === 0 ? 'Pendiente de pago' : 'Confirmada', 'Reserva de renta náutica')}
Cliente titular: ${c.client}
Servicio solicitado: ${services[c.index % 5]}
Fecha de salida: ${dateFor(c.index, 10)}
Hora de embarque: ${String(8 + c.index % 7).padStart(2, '0')}:30
Número de pasajeros: ${passengers}
Tarifa contratada: ${money(rate)}
Anticipo recibido: ${money(deposit)}
Saldo pendiente: ${money(rate - deposit)}
Estado de pago: ${c.index % 13 === 0 ? 'Pendiente' : 'Anticipo confirmado'}
Capitán asignado: ${people[(c.index + 8) % people.length]}

## Alcance de la experiencia

La reserva considera salida desde ${c.city[2]} hacia una ruta autorizada dentro de ${c.city[3]}, sujeta a las condiciones meteorológicas, instrucciones de la capitanía de puerto y criterio profesional del capitán. El itinerario preliminar incluye bienvenida de seguridad, navegación de aproximación, fondeo sólo en zonas permitidas y regreso con margen suficiente para desembarque. No se promete avistamiento de fauna, acceso a playas restringidas ni una ruta específica cuando las condiciones de seguridad requieran modificarla.

El cliente fue informado de que debe presentar identificación, lista definitiva de pasajeros y necesidades alimentarias a más tardar 48 horas antes. Menores de edad deben viajar con adulto responsable. Está prohibido abordar con sustancias ilícitas, armas, recipientes de vidrio no autorizados o equipo que supere las condiciones de peso y seguridad indicadas por la tripulación. La empresa puede negar el embarque a una persona con signos de intoxicación o conducta que ponga en riesgo a terceros.

## Pagos, cancelaciones y protección de datos

El anticipo bloquea la embarcación en el calendario operativo. El saldo se liquida antes del embarque mediante los medios autorizados. Las cancelaciones con al menos siete días de anticipación pueden reprogramarse conforme a disponibilidad; dentro de ese plazo se aplicarán cargos de preparación, catering, permisos y servicios ya contratados. Por cierre de puerto o condición meteorológica insegura, la prioridad es reprogramar y conservar el valor pagado, sin garantizar fecha inmediata.

Los datos personales se utilizarán para coordinar la salida, control de acceso, facturación y atención de emergencias. Fotografías o videos sólo se publicarán con autorización separada. El cliente reconoce que las actividades acuáticas implican riesgos inherentes y que la tripulación puede suspenderlas sin reembolso cuando exista una causa de seguridad comprobable.

## Lista de control previa

Reservas deberá confirmar manifiesto, pago, inventario de chalecos, revisión de combustible, botiquín, radio VHF y briefing de salida. El capitán firmará la bitácora y registrará cualquier cambio de pasajero o ruta. Este registro es sintético; nombres, importes, horarios y destinos son inventados para pruebas.`;
}

function contract(c) {
  const total = c.amount + 18500;
  return `# Contrato de prestación de servicios de charter\n${common(c, 'Legal y contratos', c.index % 9 === 0 ? 'En revisión legal' : 'Vigente', 'Contrato de renta de embarcación con tripulación')}
Contratante: ${c.client}
Representante de la empresa: ${c.employee}
Objeto contractual: ${services[c.index % 4]}
Importe total: ${money(total)}
Anticipo contractual: ${money(Math.round(total * 0.5))}
Fecha de servicio: ${dateFor(c.index, 14)}
Jurisdicción pactada: ${c.city[1]}, México
Anexo de seguridad: AS-${c.folio}
Anexo de privacidad: AP-${c.folio}

## Declaraciones

Mar Azul Charter & Sales, S.A. de C.V. declara, para efectos de este documento ficticio, que cuenta con capacidad operativa para coordinar la prestación del servicio con embarcación y tripulación designadas. El contratante declara que proporciona información verdadera sobre pasajeros, necesidades relevantes y finalidad del viaje. Ambas partes reconocen que el servicio depende de disposiciones de la autoridad marítima, disponibilidad de muelle, estado del mar y mantenimiento preventivo.

La embarcación se entrega únicamente con tripulación autorizada; este contrato no transmite posesión, dominio ni facultades de navegación al contratante. El capitán conserva la autoridad final en decisiones de seguridad, ruta, velocidad, fondeo, cancelación y conducta a bordo. Ninguna solicitud comercial puede obligar a la tripulación a incumplir instrucciones de capitanía, normas ambientales o límites de capacidad.

## Condiciones económicas y responsabilidad

El importe incluye los conceptos indicados en la cotización aceptada y excluye consumos, servicios especiales, daños por uso indebido y gastos de terceros que se identifiquen por separado. El anticipo se aplica a la reserva. Los ajustes deben comunicarse por escrito antes de ejecutarse. Si existe devolución procedente, se realizará al mismo medio de pago una vez que se concilien cargos verificables.

El contratante responde por daños causados intencionalmente por sus invitados, por incumplir instrucciones de seguridad o por ocultar circunstancias relevantes. La empresa mantendrá los seguros y permisos que resulten aplicables a su operación, sin que ello elimine los riesgos inherentes de navegación. En caso de emergencia, la tripulación priorizará la integridad de las personas y podrá solicitar auxilio o regresar al puerto más cercano.

## Datos, cumplimiento y firma

Los datos se tratan conforme al aviso de privacidad entregado; la información de identificación se conserva por el plazo necesario para operación, facturación y cumplimiento. Las partes se obligan a no usar el servicio para actividades ilícitas y a colaborar con verificaciones razonables de identidad u origen de fondos cuando correspondan. Cualquier controversia se procurará resolver mediante negociación documentada antes de iniciar acciones formales.

La aceptación puede realizarse mediante firma electrónica o confirmación verificable del contratante. Este archivo es una simulación de corpus y no es un contrato listo para uso legal; debe ser revisado por abogados locales antes de utilizarse en una operación real.`;
}

function maintenance(c) {
  const hours = 3 + (c.index % 8);
  return `# Orden de mantenimiento preventivo\n${common(c, 'Mantenimiento y flota', c.index % 11 === 0 ? 'Requiere autorización' : 'Cerrada', 'Orden de servicio técnico')}
Proveedor técnico: ${vendors[c.index % vendors.length]}
Técnico responsable: ${people[(c.index + 3) % people.length]}
Horas de trabajo: ${hours}
Costo estimado: ${money(12500 + c.index % 9 * 3800)}
Prioridad: ${c.index % 11 === 0 ? 'Alta' : 'Programada'}
Sistema intervenido: ${['motor principal', 'generador', 'aire acondicionado', 'bomba de achique', 'radio VHF', 'sistema de anclas'][c.index % 6]}
Próxima revisión: ${dateFor(c.index, 90)}

## Diagnóstico y alcance

Durante la inspección en ${c.city[2]}, el técnico verificó el estado visual del sistema seleccionado, niveles de fluidos, conexiones, alarmas disponibles y evidencias de corrosión o filtración. Se compararon las horas de operación con la bitácora de navegación y se revisó que las protecciones físicas estuvieran instaladas. No se autorizó ninguna maniobra de prueba sin personal de seguridad y sin disponibilidad de equipo de respuesta básico.

La intervención contempla limpieza, reapriete, sustitución de consumibles y pruebas funcionales según manual del fabricante. Cuando se detecta una pieza fuera de especificación, se etiqueta para reemplazo y se solicita cotización antes de comprometer gastos no aprobados. Las piezas retiradas deben conservarse hasta que el responsable de flota valide el cierre de la orden o indique su disposición conforme al procedimiento ambiental.

## Hallazgos, riesgo y liberación

No se liberará la embarcación para renta mientras exista una falla que afecte propulsión, comunicación, achique, extinción, navegación o integridad del casco. El capitán de la siguiente salida debe conocer cualquier observación pendiente y confirmar en la lista pre-zarpe que los elementos críticos están operativos. Los hallazgos de baja prioridad se programan en ventana de muelle para no interferir con una reserva confirmada sin aprobación de Operaciones.

El proveedor debe entregar factura, reporte fotográfico cuando aplique, número de parte y garantía de trabajo. Mantenimiento verificará el registro contra el inventario y actualizará la fecha de próxima revisión. Se prohíbe liberar lubricantes, baterías, filtros o solventes al agua; cualquier residuo se entrega a gestor autorizado.

## Cierre documental

La orden se archiva con la bitácora, aprobación de costo y firma del responsable. Este documento usa datos ficticios: no acredita mantenimiento real ni sustituye las recomendaciones del fabricante, la inspección de un perito o las obligaciones de seguridad aplicables.`;
}

function employee(c) {
  const roles = ['Capitán de charter', 'Marinero de cubierta', 'Coordinadora de reservas', 'Asesor comercial', 'Técnico de mantenimiento', 'Analista de cumplimiento'];
  const role = roles[c.index % roles.length];
  return `# Expediente de personal\n${common(c, 'Recursos Humanos', c.index % 8 === 0 ? 'Actualización pendiente' : 'Activo', 'Expediente laboral y operativo')}
Colaborador: ${c.employee}
Puesto: ${role}
Centro de trabajo: ${c.city[0]}
Supervisor directo: ${people[(c.index + 4) % people.length]}
Fecha de ingreso: ${dateFor(c.index, -420)}
Tipo de relación: Contrato por tiempo indeterminado
Certificación operativa: ${role.includes('Capitán') ? 'Licencia y libreta marítima verificadas' : 'Capacitación interna registrada'}
Evaluación vigente hasta: ${dateFor(c.index, 180)}

## Responsabilidades asignadas

La persona colaboradora deberá cumplir su descripción de puesto, las políticas de seguridad, las instrucciones de su supervisor y los procedimientos de trato al cliente. Para funciones a bordo, se exige participar en el briefing, reportar riesgos, usar equipo de protección cuando corresponda y registrar eventos relevantes en la bitácora. Para roles administrativos, se exige verificar la integridad de reservas, documentos y autorizaciones sin alterar evidencia.

El acceso a información de pasajeros, pagos y expedientes se limita a la necesidad operativa. Se prohíbe compartir contraseñas, descargar bases de datos personales en dispositivos no autorizados o divulgar itinerarios de clientes. Cualquier solicitud inusual de un tercero debe escalarse a cumplimiento antes de responder. La empresa mantiene un canal confidencial para reportar acoso, soborno, fraude, consumo de sustancias o riesgos de seguridad.

## Formación y desempeño

El plan de capacitación incluye servicio al cliente, protección de datos, prevención de lavado de dinero según el riesgo del puesto, primeros auxilios básicos y manejo de incidentes. Quienes navegan también deben conocer los límites de pasajeros, ubicación de chalecos, radio, extintores y protocolo de hombre al agua. La aprobación de una capacitación no sustituye las licencias, aptitudes médicas o certificaciones que exijan las autoridades.

La evaluación considera puntualidad, cumplimiento de listas de control, comunicación con huéspedes, cuidado de activos y calidad de registros. Las incidencias se tratan con derecho de audiencia y medidas proporcionales conforme a políticas internas y legislación aplicable. Los cambios de turno deben documentarse, especialmente si hay averías, quejas o servicios pendientes.

## Privacidad y nota de simulación

Este archivo contiene una ficha inventada para entrenamiento de búsqueda. No describe a una persona real, no debe usarse para decisiones de empleo y no reemplaza un expediente laboral, contrato, aviso de privacidad o procedimiento disciplinario válido.`;
}

function permit(c) {
  const permitTypes = ['Permiso de operación turística', 'Registro de equipo de seguridad', 'Autorización de uso de muelle', 'Constancia de manejo de residuos', 'Póliza de responsabilidad civil'];
  return `# Control de permiso y cumplimiento marítimo\n${common(c, 'Permisos y cumplimiento', c.index % 10 === 0 ? 'Por renovar' : 'Vigente', 'Expediente de permiso operativo')}
Permiso controlado: ${permitTypes[c.index % permitTypes.length]}
Autoridad o emisor: Autoridad marítima competente — registro ficticio
Número de control: PERM-${c.city[1].slice(0, 3).toUpperCase()}-${String(c.index + 100).padStart(5, '0')}
Fecha de emisión: ${dateFor(c.index, -280)}
Fecha de vencimiento: ${dateFor(c.index, 85)}
Titular registrado: Mar Azul Charter & Sales, S.A. de C.V.
Responsable de renovación: ${c.employee}
Riesgo de vencimiento: ${c.index % 10 === 0 ? 'Medio' : 'Bajo'}

## Objeto de control

Este registro centraliza el seguimiento interno de permisos, pólizas, registros y autorizaciones relacionados con la operación de ${c.yacht[0]} en ${c.city[0]}. El equipo de cumplimiento debe verificar la versión vigente directamente con el emisor y no asumir que este archivo sustituye un documento oficial. Las fechas se usan para activar recordatorios, solicitar evidencias y prevenir que se programe una salida sin soporte documental suficiente.

La revisión incluye razón social, embarcación cubierta, área geográfica, límites de pasajeros, condiciones de vigencia, pagos de derechos y obligaciones de reporte. Si el permiso contiene restricciones de horario, ruta, actividad comercial o temporada, Operaciones deberá configurarlas en el calendario. Cualquier contradicción entre un certificado, una cotización y una instrucción de autoridad se escala antes de prestar el servicio.

## Renovación y auditoría

El responsable iniciará la renovación con anticipación, reunirá comprobantes, verificará facultades de firma y conservará el acuse correspondiente. No se debe alterar una fecha, número de control o sello para resolver una reserva. Si existe vencimiento, suspensión, requerimiento de inspección o falta de documentación, la embarcación queda bloqueada para actividad afectada hasta que Legal y Operaciones documenten una liberación válida.

Una auditoría interna puede solicitar evidencia de seguros, listas de mantenimiento, manifiestos, entrenamiento de tripulación y disposición de residuos. Las observaciones se registran con dueño, fecha de compromiso y seguimiento. Los documentos originales o copias certificadas se almacenan bajo acceso limitado cuando contienen información sensible.

## Aclaración

Los permisos, autoridades y números de este archivo son ficticios. Se incluyen para probar flujos de recuperación documental y no representan una autorización real para navegar, vender o rentar embarcaciones.`;
}

function invoice(c) {
  const subtotal = c.amount;
  const tax = Math.round(subtotal * 0.16);
  return `# Factura y conciliación de servicio\n${common(c, 'Finanzas y facturación', c.index % 12 === 0 ? 'Vencida' : 'Pagada', 'Comprobante de servicio')}
Cliente facturado: ${c.client}
Concepto: ${services[c.index % services.length]}
Folio fiscal interno: FAC-${c.folio}
Subtotal: ${money(subtotal)}
Impuestos estimados: ${money(tax)}
Total facturado: ${money(subtotal + tax)}
Estado de la factura: ${c.index % 12 === 0 ? 'Vencida' : 'Pagada'}
Fecha límite de pago: ${dateFor(c.index, 25)}
Método de cobro: Transferencia bancaria verificada

## Desglose y validación

El importe corresponde a los servicios confirmados en la reserva y a los adicionales aceptados por escrito. Finanzas debe conciliar el folio de operación, la evidencia de prestación, el comprobante de pago y los datos fiscales entregados por el cliente antes de marcar el documento como pagado. Los ajustes posteriores requieren nota de crédito, autorización del responsable y explicación trazable; no se corrigen saldos únicamente por instrucción verbal.

Cuando el cliente solicite factura, la razón social y datos fiscales se tratarán como información confidencial. El equipo no debe enviar comprobantes a direcciones distintas sin verificar autorización. Las solicitudes de devolución se revisan contra la política de cancelación, cargos de terceros, bitácora de servicio y evidencia de que la cuenta receptora coincide con la persona o empresa que realizó el pago.

## Gestión de cobranza y controles

Para saldos vencidos se realiza recordatorio respetuoso, conciliación de diferencias y, si persiste el impago, escalamiento al área administrativa. Ningún colaborador puede condonar importes, aceptar efectivo fuera de caja autorizada o dividir pagos para evadir controles internos. Los pagos inusuales, de terceros o con información inconsistente se informan a cumplimiento antes de aplicarse a una operación.

La factura se conserva con su expediente comercial, reserva o contrato y los soportes de impuestos aplicables. Este documento es una simulación sin validez fiscal; cifras, folios y clientes son inventados para generar un conjunto de datos de pruebas.`;
}

function incident(c) {
  const types = ['Demora por condición meteorológica', 'Daño menor a equipo de cubierta', 'Queja de cliente por catering', 'Alerta de batería auxiliar', 'Cambio de ruta por cierre de zona', 'Lesión leve atendida a bordo'];
  return `# Reporte de incidente operativo\n${common(c, 'Seguridad y operaciones', c.index % 6 === 0 ? 'Investigación abierta' : 'Cerrado con acciones', 'Reporte de incidente')}
Tipo de incidente: ${types[c.index % types.length]}
Fecha y hora del evento: ${dateFor(c.index, 4)}T${String(9 + c.index % 8).padStart(2, '0')}:15
Reportado por: ${c.employee}
Nivel de severidad: ${c.index % 6 === 0 ? 'Moderado' : 'Bajo'}
Personas involucradas: ${c.client} y tripulación asignada
Embarcación afectada: ${c.yacht[0]}
Acción inmediata: Evaluación de seguridad y notificación al responsable de turno
Fecha objetivo de cierre: ${dateFor(c.index, 12)}

## Descripción objetiva

Durante una operación cercana a ${c.city[0]}, la tripulación identificó la situación descrita y aplicó el procedimiento inicial: reducir riesgo, informar a pasajeros de manera clara, verificar que no hubiera lesión o daño crítico y registrar hora, ubicación aproximada y decisiones tomadas. El capitán mantuvo la autoridad operativa y, cuando fue necesario, ajustó la ruta para regresar a un punto seguro. No se atribuyen responsabilidades definitivas en este reporte preliminar.

Se solicitaron declaraciones breves a las personas presentes y se preservaron fotografías, mensajes o registros técnicos pertinentes sin publicar material sensible. La atención al cliente comunicó el siguiente paso disponible, incluyendo reprogramación, revisión de consumo o llamada de seguimiento, sin prometer una compensación antes de que Finanzas y Legal evaluaran los hechos.

## Análisis y acción correctiva

El responsable de Seguridad evaluará si hubo desviación de la lista de control, falla de mantenimiento, información incompleta de reserva o factor externo. Las acciones pueden incluir capacitación, reemplazo de pieza, actualización de ruta, mejora de comunicación y revisión de proveedor. Si el evento puede ser reportable ante una autoridad o aseguradora, Legal coordinará la comunicación usando evidencia revisada.

El cierre requiere comprobar que las acciones tienen responsable, fecha y evidencia. Cualquier lesión, daño relevante, contaminación o pérdida de comunicación obliga a detener la operación afectada hasta recibir autorización formal. Este informe ficticio no documenta un accidente real ni sustituye un aviso legal o un reporte para aseguradora.`;
}

function supplier(c) {
  const vendor = vendors[c.index % vendors.length];
  return `# Evaluación de proveedor\n${common(c, 'Compras y proveedores', c.index % 7 === 0 ? 'Condicionado' : 'Aprobado', 'Evaluación y alta de proveedor')}
Proveedor: ${vendor}
Servicio o suministro: ${['combustible marino', 'mantenimiento especializado', 'catering', 'equipo de seguridad', 'limpieza de casco'][c.index % 5]}
Contacto comercial: ${people[(c.index + 11) % people.length]}
Importe anual estimado: ${money(180000 + c.index % 12 * 67000)}
Resultado de evaluación: ${c.index % 7 === 0 ? 'Aprobación condicionada' : 'Aprobación vigente'}
Fecha de próxima revisión: ${dateFor(c.index, 180)}
Contrato marco: PROV-${c.folio}

## Alcance de debida diligencia

Compras verificó que el proveedor pueda describir su servicio, emitir comprobantes y aceptar condiciones de seguridad, confidencialidad y anticorrupción. Cuando el servicio implique acceso a muelle, embarcación o información de clientes, se solicitan referencias, personal autorizado y evidencia de seguros según el nivel de riesgo. La alta no autoriza compras sin orden aprobada ni elimina las responsabilidades de supervisión del área solicitante.

La evaluación considera calidad, cumplimiento de tiempos, trazabilidad de materiales, respuesta ante emergencia y manejo de residuos. Para combustible y mantenimiento se revisan controles de derrame y documentación de entrega. Para catering se solicitan requisitos de higiene y una lista de alérgenos. Para servicios técnicos, se requiere que el proveedor identifique piezas y garantías aplicables.

## Condiciones de relación

El proveedor se obliga a no ofrecer incentivos indebidos, no utilizar datos personales fuera del servicio y notificar conflictos de interés. Mar Azul puede suspender órdenes si detecta incumplimiento, documentación incompleta o riesgo para clientes, personal, ambiente o activos. Las modificaciones de precio deben comunicarse antes de realizar el trabajo y quedar registradas en la orden correspondiente.

Este expediente es sintético. El nombre comercial, contacto, importes y número contractual son inventados y no deben utilizarse para contratar, facturar o acreditar una relación real.`;
}

function inventory(c) {
  const item = ['Chaleco salvavidas infantil', 'Radio VHF portátil', 'Bengala de señalización', 'Kit de snorkel', 'Extintor marino', 'Botiquín de primeros auxilios'][c.index % 6];
  return `# Registro de inventario de seguridad y hospitalidad\n${common(c, 'Inventario y activos', c.index % 5 === 0 ? 'Revisión requerida' : 'Disponible', 'Control de inventario')}
Activo controlado: ${item}
Código de activo: INV-${c.yacht[4]}-${String(c.index + 1).padStart(4, '0')}
Cantidad registrada: ${2 + c.index % 16}
Estado físico: ${c.index % 5 === 0 ? 'Pendiente de inspección' : 'Operativo'}
Ubicación a bordo: ${['gaveta de seguridad', 'cabina principal', 'compartimiento de popa', 'bodega seca'][c.index % 4]}
Última inspección: ${dateFor(c.index, -18)}
Responsable de conteo: ${c.employee}

## Procedimiento de custodia

El activo se cuenta antes y después de cada salida cuando su uso se relaciona con seguridad, pasajeros o inventario cobrable. La persona responsable debe registrar faltantes, daños, caducidades y reemplazos sin alterar el conteo para cerrar una operación. Los elementos de seguridad vencidos, contaminados o deteriorados se separan de los disponibles y se reportan de inmediato al responsable de flota.

Las listas de inventario ayudan a confirmar que la capacidad de la embarcación, los chalecos y el equipo de comunicación son consistentes con el manifiesto. Ningún artículo crítico se sustituye por uno no homologado sólo para cumplir una cantidad. La verificación física prevalece sobre el registro cuando exista una diferencia; después se investiga la causa y se corrige la base documental con evidencia.

## Seguimiento

Compras coordinará reposiciones aprobadas y Mantenimiento verificará instalación cuando aplique. En artículos prestados a clientes se anotará condición de entrega y retorno. Este registro es una muestra ficticia para indexación y no certifica que un equipo real sea apto para navegación o emergencias.`;
}

function compliance(c) {
  const themes = ['Protección de datos personales', 'Prevención de fraude y pagos inusuales', 'Anticorrupción y regalos', 'Revisión de beneficiario controlador', 'Gestión de quejas y derechos de usuarios'];
  return `# Memorando de cumplimiento\n${common(c, 'Cumplimiento corporativo', c.index % 4 === 0 ? 'Seguimiento activo' : 'Emitido', 'Memorando de política interna')}
Tema de control: ${themes[c.index % themes.length]}
Destinatario: Personal de ventas, reservas, operaciones y finanzas
Emisor: ${c.employee}
Fecha de aplicación: ${dateFor(c.index, 2)}
Fecha de revisión: ${dateFor(c.index, 182)}
Nivel de riesgo: Medio
Canal de reporte: cumplimiento@marazul-ejemplo.mx

## Instrucción operativa

Todo el personal debe utilizar información de clientes, tripulación y proveedores sólo para fines legítimos de servicio. Las solicitudes que involucren pagos de terceros, cambios repentinos de beneficiario, documentación incompleta, presión para omitir controles o trato preferencial a una autoridad deben detenerse y escalarse. La rapidez comercial no justifica ignorar políticas, ocultar hechos o crear evidencia retrospectiva.

Los equipos deben conservar los registros mínimos de aceptación, cotización, reserva, contrato, factura e incidente. Cuando una persona solicite acceso, corrección o eliminación de datos, se canalizará al responsable designado y no se modificará información de manera informal. Las denuncias de conducta irregular se atienden sin represalias y con acceso restringido a quienes necesiten investigar.

## Supervisión y mejora

Cumplimiento revisará una muestra de expedientes, identificará patrones y propondrá capacitación o controles adicionales. Los responsables de área deben responder a hallazgos con fecha, dueño y evidencia de cierre. Si una obligación depende de una ley, permiso o contrato específico, se solicitará asesoría profesional; este memorando no pretende interpretar normativa vigente.

El presente texto, dirección de correo y datos empresariales son simulados. Se incluye para probar que Omega pueda recuperar políticas extensas junto con documentos operativos y financieros.`;
}

const categories = [
  ['01_ventas', 100, sales], ['02_reservas_charter', 140, reservation], ['03_contratos', 80, contract],
  ['04_mantenimiento', 70, maintenance], ['05_personal', 45, employee], ['06_permisos_cumplimiento', 45, permit],
  ['07_facturas', 45, invoice], ['08_incidentes', 30, incident], ['09_proveedores', 20, supplier],
  ['10_inventario', 15, inventory], ['11_politicas', 10, compliance],
];

await rm(root, { recursive: true, force: true });
let globalIndex = 0;
for (const [folder, count, render] of categories) {
  const dir = join(root, folder);
  await mkdir(dir, { recursive: true });
  for (let index = 0; index < count; index += 1) {
    const c = context(globalIndex++);
    const filename = `${String(index + 1).padStart(3, '0')}_${c.folio.toLowerCase()}_${folder}.md`;
    await writeFile(join(dir, filename), `${render(c).trim()}\n`, 'utf8');
  }
}

console.log(`Corpus creado en ${root}: ${globalIndex} documentos Markdown.`);
