// Allocation fixtures for the FLIP mock server. Summaries drive the list; the
// details map is the get_allocation lookup (each carries a wallet_operations
// timeline + any failures). Federation-centric model (post `delete request_id`):
// the federation is the allocation identity; the summary carries per-source
// status and the detail carries per-item `item_statuses`. Keep aligned with
// @operator-ui/types.

import type {
  AdminAllocationDetail,
  AdminAllocationSummary,
  FederationId
} from '@operator-ui/types';

// Shared mock provider identity for the public allocation-status struct.
const PROVIDER = '03prov'.padEnd(66, '0');
const GATEWAY_ID = 'gw-signet-01';
const GATEWAY_NAME = 'Mock Signet Gateway';

// details_payload_hash is a 32-byte array on the wire (public.rs `[u8; 32]`).
const detailsHash = (seed: number): number[] =>
  Array.from({ length: 32 }, (_, i) => (seed * 7 + i) % 256);

export const allocationSummaries: AdminAllocationSummary[] = [
  {
    federation_id: 'fed-0001',
    gateway_status: 'completed',
    stability_pool_status: null,
    committed_amount: 2_500_000,
    created_at: 1721400000,
    updated_at: 1721403600
  },
  {
    federation_id: 'fed-0002',
    gateway_status: 'running',
    stability_pool_status: null,
    committed_amount: 1_000_000,
    created_at: 1721410000,
    updated_at: 1721410800
  },
  {
    federation_id: 'fed-0003',
    gateway_status: 'pending',
    stability_pool_status: null,
    committed_amount: 500_000,
    created_at: 1721420000,
    updated_at: 1721420000
  },
  {
    federation_id: 'fed-0004',
    gateway_status: 'failed',
    stability_pool_status: null,
    committed_amount: 750_000,
    created_at: 1721430000,
    updated_at: 1721430900
  }
];

export const allocationDetails: Record<FederationId, AdminAllocationDetail> = {
  'fed-0001': {
    federation_id: 'fed-0001',
    status: {
      details_payload_hash: detailsHash(1),
      provider_pubkey: PROVIDER,
      item_statuses: [
        {
          target: {
            gateway: {
              item_id: 'item-0001',
              gateway_id: GATEWAY_ID,
              gateway_name: GATEWAY_NAME,
              amount: 2_500_000
            }
          },
          status: 'completed',
          fulfilled_amount: 2_500_000,
          completion_evidence: null,
          failure: null,
          updated_at: 1721403600
        }
      ]
    },
    wallet_operations: [
      {
        operation_id: 'op-0001',
        operation_type: 'deposit',
        amount: 2_500_000,
        address: 'tb1qmockdeposit0001',
        txid: 'a1b2c3deposit0001',
        tx_vout: 0,
        status: 'confirmed',
        confirmation_count: 6,
        federation_id: 'fed-0001',
        item_id: 'item-0001',
        created_at: 1721400000,
        updated_at: 1721401200,
        failure: null
      },
      {
        operation_id: 'op-0002',
        operation_type: 'gateway_funding',
        amount: 2_500_000,
        address: null,
        txid: 'd4e5f6gateway0001',
        status: 'completed',
        confirmation_count: 3,
        federation_id: 'fed-0001',
        item_id: 'item-0001',
        created_at: 1721401200,
        updated_at: 1721403600,
        failure: null
      }
    ],
    failures: []
  },
  'fed-0002': {
    federation_id: 'fed-0002',
    status: {
      details_payload_hash: detailsHash(2),
      provider_pubkey: PROVIDER,
      item_statuses: [
        {
          target: {
            gateway: {
              item_id: 'item-0002',
              gateway_id: GATEWAY_ID,
              gateway_name: GATEWAY_NAME,
              amount: 1_000_000
            }
          },
          status: 'running',
          fulfilled_amount: null,
          completion_evidence: null,
          failure: null,
          updated_at: 1721410800
        }
      ]
    },
    wallet_operations: [
      {
        operation_id: 'op-0003',
        operation_type: 'deposit',
        amount: 1_000_000,
        address: 'tb1qmockdeposit0002',
        txid: 'a1b2c3deposit0002',
        tx_vout: 1,
        status: 'confirmed',
        confirmation_count: 4,
        federation_id: 'fed-0002',
        item_id: 'item-0002',
        created_at: 1721410000,
        updated_at: 1721410400,
        failure: null
      },
      {
        operation_id: 'op-0004',
        operation_type: 'gateway_funding',
        amount: 1_000_000,
        address: null,
        txid: null,
        status: 'pending',
        confirmation_count: 0,
        federation_id: 'fed-0002',
        item_id: 'item-0002',
        created_at: 1721410400,
        updated_at: 1721410800,
        failure: null
      }
    ],
    failures: []
  },
  'fed-0003': {
    federation_id: 'fed-0003',
    status: {
      details_payload_hash: detailsHash(3),
      provider_pubkey: PROVIDER,
      item_statuses: [
        {
          target: {
            gateway: {
              item_id: 'item-0003',
              gateway_id: GATEWAY_ID,
              gateway_name: GATEWAY_NAME,
              amount: 500_000
            }
          },
          status: 'pending',
          fulfilled_amount: null,
          completion_evidence: null,
          failure: null,
          updated_at: 1721420000
        }
      ]
    },
    wallet_operations: [
      {
        operation_id: 'op-0005',
        operation_type: 'deposit',
        amount: 500_000,
        address: 'tb1qmockdeposit0003',
        txid: null,
        status: 'pending',
        confirmation_count: 0,
        federation_id: 'fed-0003',
        item_id: 'item-0003',
        created_at: 1721420000,
        updated_at: 1721420000,
        failure: null
      }
    ],
    failures: []
  },
  'fed-0004': {
    federation_id: 'fed-0004',
    status: {
      details_payload_hash: detailsHash(4),
      provider_pubkey: PROVIDER,
      item_statuses: [
        {
          target: {
            gateway: {
              item_id: 'item-0004',
              gateway_id: GATEWAY_ID,
              gateway_name: GATEWAY_NAME,
              amount: 750_000
            }
          },
          status: 'failed',
          fulfilled_amount: null,
          completion_evidence: null,
          failure: {
            code: 'gateway_attach_failed',
            reason: 'gateway did not accept the funding operation'
          },
          updated_at: 1721430900
        }
      ]
    },
    wallet_operations: [
      {
        operation_id: 'op-0006',
        operation_type: 'deposit',
        amount: 750_000,
        address: 'tb1qmockdeposit0004',
        txid: 'a1b2c3deposit0004',
        tx_vout: 0,
        status: 'confirmed',
        confirmation_count: 6,
        federation_id: 'fed-0004',
        item_id: 'item-0004',
        created_at: 1721430000,
        updated_at: 1721430300,
        failure: null
      },
      {
        operation_id: 'op-0007',
        operation_type: 'gateway_funding',
        amount: 750_000,
        address: null,
        txid: null,
        status: 'failed',
        confirmation_count: 0,
        federation_id: 'fed-0004',
        item_id: 'item-0004',
        created_at: 1721430300,
        updated_at: 1721430900,
        failure: {
          code: 'gateway_unreachable',
          message: 'gateway did not accept the funding operation',
          occurred_at: 1721430900,
          federation_id: 'fed-0004',
          item_id: 'item-0004'
        }
      }
    ],
    failures: [
      {
        code: 'gateway_unreachable',
        message: 'gateway did not accept the funding operation',
        occurred_at: 1721430900,
        federation_id: 'fed-0004',
        item_id: 'item-0004'
      }
    ]
  }
};
