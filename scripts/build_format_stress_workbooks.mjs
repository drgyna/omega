import fs from 'node:fs/promises';
import path from 'node:path';
import { SpreadsheetFile, Workbook } from '@oai/artifact-tool';

const root = path.join(process.cwd(), 'corpus-prueba-formatos-extremos', '04_excel_xlsx');
const cities = ['Ciudad de México', 'Guadalajara', 'Monterrey', 'Puebla', 'Querétaro', 'Mérida'];
const states = ['Activo', 'En revisión', 'Cerrado'];

await fs.mkdir(root, { recursive: true });

for (let index = 0; index < 6; index += 1) {
  const workbook = Workbook.create();
  const sheet = workbook.worksheets.add('Registros');
  sheet.showGridLines = false;
  sheet.getRange('A1:G1').merge();
  sheet.getRange('A1').values = [[`Reporte operativo sintético ${index + 1}`]];
  sheet.getRange('A1:G1').format = {
    fill: '#1F4D78',
    font: { bold: true, color: '#FFFFFF', size: 16 },
    horizontalAlignment: 'center', verticalAlignment: 'center',
  };
  sheet.getRange('A1:G1').format.rowHeight = 28;
  sheet.getRange('A3:G3').values = [[
    'Folio', 'Tipo de documento', 'Estado', 'Ciudad base', 'Fecha de registro', 'Importe total', 'Observación',
  ]];
  sheet.getRange('A3:G3').format = {
    fill: '#E8EEF5', font: { bold: true, color: '#1F4D78' },
    horizontalAlignment: 'center', wrapText: true,
    borders: { preset: 'outside', style: 'thin', color: '#B7C4D4' },
  };
  const rows = Array.from({ length: 10 }, (_, row) => {
    const sequence = 200 + index * 10 + row;
    return [
      `XLS-26-${String(sequence).padStart(4, '0')}`,
      row % 2 === 0 ? 'Registro de inventario' : 'Informe de operación',
      states[(index + row) % states.length],
      cities[(index * 2 + row) % cities.length],
      new Date(Date.UTC(2026, row % 8, 5 + row)),
      5400 + sequence * 19,
      'Datos ficticios para validar tablas, fechas, importes y evidencia de hojas de cálculo.',
    ];
  });
  sheet.getRange('A4:G13').values = rows;
  sheet.getRange('A4:G13').format = {
    borders: { preset: 'inside', style: 'thin', color: '#D9E1EA' },
    verticalAlignment: 'top', wrapText: true,
  };
  sheet.getRange('E4:E13').format.numberFormat = 'yyyy-mm-dd';
  sheet.getRange('F4:F13').format.numberFormat = '"$"#,##0.00';
  sheet.getRange('F15').formulas = [['=SUM(F4:F13)']];
  sheet.getRange('E15').values = [['Total de importes']];
  sheet.getRange('E15:F15').format = {
    fill: '#F2F4F7', font: { bold: true, color: '#1F4D78' },
    borders: { preset: 'outside', style: 'thin', color: '#B7C4D4' },
  };
  sheet.getRange('F15').format.numberFormat = '"$"#,##0.00';
  for (const [range, width] of [['A:A', 16], ['B:B', 24], ['C:C', 16], ['D:D', 21], ['E:E', 18], ['F:F', 16], ['G:G', 52]]) {
    sheet.getRange(range).format.columnWidth = width;
  }
  sheet.freezePanes.freezeRows(3);
  const output = await SpreadsheetFile.exportXlsx(workbook);
  await output.save(path.join(root, `${String(index + 1).padStart(3, '0')}-reporte-operativo.xlsx`));
}

console.log(`Creados 6 XLSX en ${root}`);
