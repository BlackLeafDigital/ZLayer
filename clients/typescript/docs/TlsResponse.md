
# TlsResponse

Response for `GET /api/v1/proxy/tls`.

## Properties

Name | Type
------------ | -------------
`acmeAvailable` | boolean
`acmeEmail` | string
`certificates` | [Array&lt;CertInfo&gt;](CertInfo.md)
`total` | number

## Example

```typescript
import type { TlsResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "acmeAvailable": null,
  "acmeEmail": null,
  "certificates": null,
  "total": null,
} satisfies TlsResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as TlsResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


