import type { ReactNode } from 'react';
import styles from './DataTable.module.css';

export interface Column<Row> {
  key: string;
  header: string;
  render: (row: Row) => ReactNode;
}

interface DataTableProps<Row> {
  columns: Column<Row>[];
  rows: Row[];
  rowKey: (row: Row) => string;
}

export const DataTable = <Row,>({ columns, rows, rowKey }: DataTableProps<Row>) => (
  <table className={styles.table}>
    <thead>
      <tr className={styles.headRow}>
        {columns.map((col) => (
          <th key={col.key} className={styles.headCell}>
            {col.header}
          </th>
        ))}
      </tr>
    </thead>

    <tbody>
      {rows.map((row) => (
        <tr key={rowKey(row)} className={styles.bodyRow}>
          {columns.map((col) => (
            <td key={col.key} className={styles.bodyCell}>
              {col.render(row)}
            </td>
          ))}
        </tr>
      ))}
    </tbody>
  </table>
);
