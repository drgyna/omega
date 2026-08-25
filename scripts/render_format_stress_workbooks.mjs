import fs from 'node:fs/promises';
import path from 'node:path';
import { FileBlob, SpreadsheetFile } from '@oai/artifact-tool';

const source = path.join(process.cwd(), 'corpus-prueba-formatos-extremos', '04_excel_xlsx');
const output = path.join('/private/tmp', 'omega-format-stress-qa', 'xlsx');
await fs.mkdir(output, { recursive: true });
const files = (await fs.readdir(source)).filter((file) => file.endsWith('.xlsx')).sort();
for (const file of files) {
  const input = await FileBlob.load(path.join(source, file));
  const workbook = await SpreadsheetFile.importXlsx(input);
  const preview = await workbook.render({ sheetName: 'Registros', range: 'A1:G15', scale: 1.2, format: 'png' });
  await fs.writeFile(path.join(output, `${path.basename(file, '.xlsx')}.png`), new Uint8Array(await preview.arrayBuffer()));
}
console.log(`Renderizados ${files.length} XLSX en ${output}`);
