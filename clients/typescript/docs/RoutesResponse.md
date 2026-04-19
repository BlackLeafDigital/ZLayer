
# RoutesResponse

Response for `GET /api/v1/proxy/routes`.

## Properties

Name | Type
------------ | -------------
`routes` | [Array&lt;RouteInfo&gt;](RouteInfo.md)
`total` | number

## Example

```typescript
import type { RoutesResponse } from '@zlayer/client'

// TODO: Update the object below with actual values
const example = {
  "routes": null,
  "total": null,
} satisfies RoutesResponse

console.log(example)

// Convert the instance to a JSON string
const exampleJSON: string = JSON.stringify(example)
console.log(exampleJSON)

// Parse the JSON string back to an object
const exampleParsed = JSON.parse(exampleJSON) as RoutesResponse
console.log(exampleParsed)
```

[[Back to top]](#) [[Back to API list]](../README.md#api-endpoints) [[Back to Model list]](../README.md#models) [[Back to README]](../README.md)


