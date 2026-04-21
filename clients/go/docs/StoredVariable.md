# StoredVariable

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | When the variable was created. | 
**Id** | **string** | UUID identifier. | 
**Name** | **string** | Variable name (e.g. &#x60;\&quot;APP_VERSION\&quot;&#x60;, &#x60;\&quot;LOG_LEVEL\&quot;&#x60;). Unique within a given scope. | 
**Scope** | Pointer to **NullableString** | Scope: project id or &#x60;None&#x60; for global. | [optional] 
**UpdatedAt** | **string** | When the variable was last updated. | 
**Value** | **string** | Plaintext variable value. | 

## Methods

### NewStoredVariable

`func NewStoredVariable(createdAt string, id string, name string, updatedAt string, value string, ) *StoredVariable`

NewStoredVariable instantiates a new StoredVariable object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStoredVariableWithDefaults

`func NewStoredVariableWithDefaults() *StoredVariable`

NewStoredVariableWithDefaults instantiates a new StoredVariable object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *StoredVariable) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *StoredVariable) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *StoredVariable) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetId

`func (o *StoredVariable) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *StoredVariable) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *StoredVariable) SetId(v string)`

SetId sets Id field to given value.


### GetName

`func (o *StoredVariable) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *StoredVariable) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *StoredVariable) SetName(v string)`

SetName sets Name field to given value.


### GetScope

`func (o *StoredVariable) GetScope() string`

GetScope returns the Scope field if non-nil, zero value otherwise.

### GetScopeOk

`func (o *StoredVariable) GetScopeOk() (*string, bool)`

GetScopeOk returns a tuple with the Scope field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetScope

`func (o *StoredVariable) SetScope(v string)`

SetScope sets Scope field to given value.

### HasScope

`func (o *StoredVariable) HasScope() bool`

HasScope returns a boolean if a field has been set.

### SetScopeNil

`func (o *StoredVariable) SetScopeNil(b bool)`

 SetScopeNil sets the value for Scope to be an explicit nil

### UnsetScope
`func (o *StoredVariable) UnsetScope()`

UnsetScope ensures that no value is present for Scope, not even an explicit nil
### GetUpdatedAt

`func (o *StoredVariable) GetUpdatedAt() string`

GetUpdatedAt returns the UpdatedAt field if non-nil, zero value otherwise.

### GetUpdatedAtOk

`func (o *StoredVariable) GetUpdatedAtOk() (*string, bool)`

GetUpdatedAtOk returns a tuple with the UpdatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUpdatedAt

`func (o *StoredVariable) SetUpdatedAt(v string)`

SetUpdatedAt sets UpdatedAt field to given value.


### GetValue

`func (o *StoredVariable) GetValue() string`

GetValue returns the Value field if non-nil, zero value otherwise.

### GetValueOk

`func (o *StoredVariable) GetValueOk() (*string, bool)`

GetValueOk returns a tuple with the Value field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetValue

`func (o *StoredVariable) SetValue(v string)`

SetValue sets Value field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


