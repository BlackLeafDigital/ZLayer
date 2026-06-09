# NodeSelector

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Labels** | Pointer to **map[string]string** | Required labels that nodes must have (all must match) | [optional] 
**PreferLabels** | Pointer to **map[string]string** | Preferred labels (soft constraint, nodes with these are preferred) | [optional] 

## Methods

### NewNodeSelector

`func NewNodeSelector() *NodeSelector`

NewNodeSelector instantiates a new NodeSelector object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeSelectorWithDefaults

`func NewNodeSelectorWithDefaults() *NodeSelector`

NewNodeSelectorWithDefaults instantiates a new NodeSelector object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetLabels

`func (o *NodeSelector) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *NodeSelector) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *NodeSelector) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *NodeSelector) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetPreferLabels

`func (o *NodeSelector) GetPreferLabels() map[string]string`

GetPreferLabels returns the PreferLabels field if non-nil, zero value otherwise.

### GetPreferLabelsOk

`func (o *NodeSelector) GetPreferLabelsOk() (*map[string]string, bool)`

GetPreferLabelsOk returns a tuple with the PreferLabels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPreferLabels

`func (o *NodeSelector) SetPreferLabels(v map[string]string)`

SetPreferLabels sets PreferLabels field to given value.

### HasPreferLabels

`func (o *NodeSelector) HasPreferLabels() bool`

HasPreferLabels returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


