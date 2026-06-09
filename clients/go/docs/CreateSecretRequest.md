# CreateSecretRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Name** | **string** | The name of the secret. | 
**NodeAffinity** | Pointer to [**NullableNodeAffinity**](NodeAffinity.md) | Optional per-secret node affinity. &#x60;None&#x60; (default) &#x3D; any node may host the decryptable form. When set, only matching nodes receive a wrap of the DEK material for this row, and the API gate filters reads accordingly. Ignored on standalone (non-clustered) daemons. | [optional] 
**Scope** | Pointer to **NullableString** | Optional explicit scope (legacy form). Mutually exclusive with the &#x60;?environment&#x3D;&#x60; query parameter. | [optional] 
**Value** | **string** | The secret value (will be encrypted at rest). | 

## Methods

### NewCreateSecretRequest

`func NewCreateSecretRequest(name string, value string, ) *CreateSecretRequest`

NewCreateSecretRequest instantiates a new CreateSecretRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateSecretRequestWithDefaults

`func NewCreateSecretRequestWithDefaults() *CreateSecretRequest`

NewCreateSecretRequestWithDefaults instantiates a new CreateSecretRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetName

`func (o *CreateSecretRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateSecretRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateSecretRequest) SetName(v string)`

SetName sets Name field to given value.


### GetNodeAffinity

`func (o *CreateSecretRequest) GetNodeAffinity() NodeAffinity`

GetNodeAffinity returns the NodeAffinity field if non-nil, zero value otherwise.

### GetNodeAffinityOk

`func (o *CreateSecretRequest) GetNodeAffinityOk() (*NodeAffinity, bool)`

GetNodeAffinityOk returns a tuple with the NodeAffinity field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeAffinity

`func (o *CreateSecretRequest) SetNodeAffinity(v NodeAffinity)`

SetNodeAffinity sets NodeAffinity field to given value.

### HasNodeAffinity

`func (o *CreateSecretRequest) HasNodeAffinity() bool`

HasNodeAffinity returns a boolean if a field has been set.

### SetNodeAffinityNil

`func (o *CreateSecretRequest) SetNodeAffinityNil(b bool)`

 SetNodeAffinityNil sets the value for NodeAffinity to be an explicit nil

### UnsetNodeAffinity
`func (o *CreateSecretRequest) UnsetNodeAffinity()`

UnsetNodeAffinity ensures that no value is present for NodeAffinity, not even an explicit nil
### GetScope

`func (o *CreateSecretRequest) GetScope() string`

GetScope returns the Scope field if non-nil, zero value otherwise.

### GetScopeOk

`func (o *CreateSecretRequest) GetScopeOk() (*string, bool)`

GetScopeOk returns a tuple with the Scope field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetScope

`func (o *CreateSecretRequest) SetScope(v string)`

SetScope sets Scope field to given value.

### HasScope

`func (o *CreateSecretRequest) HasScope() bool`

HasScope returns a boolean if a field has been set.

### SetScopeNil

`func (o *CreateSecretRequest) SetScopeNil(b bool)`

 SetScopeNil sets the value for Scope to be an explicit nil

### UnsetScope
`func (o *CreateSecretRequest) UnsetScope()`

UnsetScope ensures that no value is present for Scope, not even an explicit nil
### GetValue

`func (o *CreateSecretRequest) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *CreateSecretRequest) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *CreateSecretRequest) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


